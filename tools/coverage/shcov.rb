# frozen_string_literal: true

# Shell line coverage, measured with bashcov — its xtrace parser and its lexer —
# over every bash process a project's test command starts, however deep in the
# process tree. tools/coverage/shcov.sh is the entry point; read it first.
#
#   shcov.rb run <project> <out.json> -- <command...>
#   shcov.rb report <floor> <resultset.json...>
#
# `run` differs from the stock `bashcov <command>` in how the trace reaches it.
# bashcov hands one pipe descriptor to the command it starts, but the scripts here
# run several processes down (cargo, nextest, a test binary, bun), and a parent
# that closes inherited descriptors leaves a child bash writing its trace to
# stderr, which the journeys assert on. So each bash process opens a file of its
# own instead: BASH_ENV (which every non-interactive bash reads) points at a
# snippet that opens a per-process trace file, sets bashcov's PS4 and turns on
# xtrace. The traces are parsed after the command exits, with bashcov's Xtrace.
#
# Journeys often copy a script verbatim into a scratch root and run the copy, then
# delete it. The snippet records, while the copy still exists, which scratch files
# are byte-identical to which repository script (by SHA-256), and hits on a copy
# count for the script it is a copy of. A stand-in that only shares a script's name
# has different bytes, so it never counts.
#
# A resultset is SimpleCov's .resultset.json shape with repository-relative
# paths and raw hit counts: { name => { "coverage" => { path => { "lines" => [...] } } } }.

require "bashcov"
require "bashcov/field_stream"
require "bashcov/lexer"
require "bashcov/xtrace"
require "digest"
require "fileutils"
require "json"
require "pathname"
require "shellwords"
require "tmpdir"

ROOT = Pathname.new(File.expand_path("../..", __dir__)).freeze

def die(message, status = 2)
  warn "shcov: #{message}"
  exit status
end

# Every shell script in the tree, repository-relative — the one discovery the
# format and lint targets are also held to (tools/shell/shell.sh files).
def shell_files
  out = IO.popen(["bash", ROOT.join("tools/shell/shell.sh").to_s, "files"], chdir: ROOT.to_s, &:read)
  die("'tools/shell/shell.sh files' failed; fix it before measuring coverage.", 1) unless $?.success?
  out.split("\n").reject(&:empty?)
end

# Resolves the script a trace line names. bashcov resolves a relative
# BASH_SOURCE against the process's working directories, newest first, by asking
# whether the file exists — which a deleted scratch copy no longer does, so a
# copy recorded in the map counts as existing too.
class TraceParser < Bashcov::Xtrace
  # bashcov keeps the PS4 delimiter on the class it is read from; share the one
  # the snippet's PS4 was built with rather than drawing a new one here.
  def self.delimiter
    Bashcov::Xtrace.delimiter
  end

  def initialize(trace, copies)
    super(Bashcov::FieldStream.new)
    @write.close
    @read.close
    @read = File.open(trace, "r:UTF-8")
    @copies = copies
  end

  private

  def find_script(bash_source)
    candidates =
      if bash_source.absolute?
        [bash_source.cleanpath]
      else
        @pwd_stack.reverse.map { |wd| (wd + bash_source).cleanpath }
      end
    candidates.find { |c| @copies.key?(c.to_s) || c.file? } || candidates.first || bash_source
  end
end

def run(project, out, command)
  die("run needs a project name, an output path and a command after --") if project.to_s.empty? || out.to_s.empty? || command.empty?
  files = shell_files
  hashes = files.to_h { |f| [f, Digest::SHA256.file(ROOT.join(f)).hexdigest] }
  sha = %w[sha256sum shasum].map { |t| `command -v #{t} 2>/dev/null`.strip }.find { |p| !p.empty? }
  die("no sha256sum or shasum on PATH; install coreutils and re-run.", 1) unless sha
  sha_cmd = sha.end_with?("shasum") ? "#{sha.shellescape} -a 256" : sha.shellescape

  run_dir = Pathname.new(Dir.mktmpdir("shcov-#{project}-", ROOT.join("target").tap(&:mkpath).to_s))
  table = hashes.map { |f, h| "#{f.shellescape} #{h}" }.join(" ")
  # The BASH_ENV this run replaces still runs first. Its path is written in
  # literally — a snippet that read it from the environment would, inside a
  # nested run, source itself forever — and SHCOV_PARENT_BASH_ENV only says to:
  # a journey that clears the environment gets this snippet alone.
  parent = ENV["BASH_ENV"].to_s
  chain = parent.empty? ? "" : %(if [ -n "${SHCOV_PARENT_BASH_ENV:-}" ]; then . #{parent.shellescape}; fi)
  # Before `set -x`, so none of this is traced; its one external command is the
  # SHA-256 tool, by the absolute path resolved here, so a journey whose PATH holds
  # nothing but stand-ins still records its copies.
  run_dir.join("bash_env.sh").write(<<~BASH)
    #{chain}
    if [ -d #{run_dir.to_s.shellescape} ]; then
      case "$0" in /*) __shcov_self=$0 ;; *) __shcov_self=$PWD/$0 ;; esac
      case "$__shcov_self" in
        #{ROOT.to_s.shellescape}/*) ;;
        *)
          __shcov_tab=(#{table})
          for ((__shcov_i = 0; __shcov_i < ${#__shcov_tab[@]}; __shcov_i += 2)); do
            __shcov_rel=${__shcov_tab[__shcov_i]}
            case "$__shcov_self" in */"$__shcov_rel") ;; *) continue ;; esac
            __shcov_base=${__shcov_self%/"$__shcov_rel"}
            for ((__shcov_j = 0; __shcov_j < ${#__shcov_tab[@]}; __shcov_j += 2)); do
              __shcov_copy=$__shcov_base/${__shcov_tab[__shcov_j]}
              [ -f "$__shcov_copy" ] || continue
              __shcov_sum=$(#{sha_cmd} "$__shcov_copy" 2>/dev/null) || continue
              [ "${__shcov_sum%% *}" = "${__shcov_tab[__shcov_j + 1]}" ] || continue
              printf '%s\\t%s\\n' "$__shcov_copy" "${__shcov_tab[__shcov_j]}" >>#{run_dir.join("copies").to_s.shellescape}
            done
          done
          ;;
      esac
      unset __shcov_self __shcov_tab __shcov_i __shcov_j __shcov_rel __shcov_base __shcov_copy __shcov_sum
      exec {__shcov_fd}>>#{run_dir.to_s.shellescape}/trace.$$.$RANDOM$RANDOM
      BASH_XTRACEFD=$__shcov_fd
      PS4=#{Bashcov::Xtrace.ps4.shellescape}
      set -x
    fi
  BASH

  # SHCOV_BASH_ENV is for a journey that clears its child's environment: it
  # passes this one value back as BASH_ENV so the child is still measured.
  env = {
    "BASH_ENV" => run_dir.join("bash_env.sh").to_s,
    "SHCOV_BASH_ENV" => run_dir.join("bash_env.sh").to_s,
    "SHCOV_PARENT_BASH_ENV" => parent.empty? ? nil : "1",
  }
  pid = Process.spawn(env, *command)
  Process.wait(pid)
  status = $?

  copies = {}
  copies_file = run_dir.join("copies")
  copies_file.each_line { |l| c, r = l.chomp.split("\t", 2); copies[c] = r } if copies_file.file?

  hits = Hash.new { |h, k| h[k] = [] }
  Dir.glob(run_dir.join("trace.*").to_s).each do |trace|
    begin
      traced = TraceParser.new(trace, copies).read
    rescue Bashcov::XtraceError => e
      warn "shcov: #{File.basename(trace)}: #{e.message}; keeping the lines read before it."
      traced = e.files
    end
    traced.each do |path, lines|
      key = path.to_s
      rel = copies[key] || (key.start_with?("#{ROOT}/") ? key.delete_prefix("#{ROOT}/") : nil)
      next unless rel && hashes.key?(rel)
      lines.each_with_index { |n, i| hits[rel][i] = hits[rel][i].to_i + n if n }
    end
  end
  FileUtils.rm_rf(run_dir)

  out_path = Pathname.new(out)
  out_path = ROOT.join(out_path) unless out_path.absolute?
  out_path.dirname.mkpath
  coverage = hits.sort.to_h { |rel, lines| [rel, { "lines" => lines }] }
  out_path.write(JSON.pretty_generate({ project => { "coverage" => coverage, "timestamp" => Time.now.to_i } }))
  exit(status.exitstatus || 1)
end

# A record as `run` writes it: one or more runs, each mapping paths to line
# arrays of hit counts (a non-negative integer, or null for no data). Anything
# else is refused rather than merged, so a damaged record cannot move the number.
def record?(data)
  data.is_a?(Hash) && !data.empty? && data.each_value.all? do |run|
    run.is_a?(Hash) && run["coverage"].is_a?(Hash) && run["coverage"].all? do |rel, cov|
      rel.is_a?(String) && cov.is_a?(Hash) && cov["lines"].is_a?(Array) \
        && cov["lines"].all? { |n| n.nil? || (n.is_a?(Integer) && !n.negative?) }
    end
  end
end

def report(floor_s, resultsets)
  die("report needs the floor (a whole percentage) and at least one resultset") if resultsets.empty?
  die("the floor must be a whole percentage from 0 to 100 (got '#{floor_s}')") unless floor_s.match?(/\A(100|[1-9]?[0-9])\z/)
  floor = floor_s.to_i
  files = shell_files
  merged = Hash.new { |h, k| h[k] = [] }
  resultsets.each do |rs|
    path = Pathname.new(rs)
    path = ROOT.join(path) unless path.absolute?
    die("no resultset at #{rs}; run that project's test target first (it writes it).", 1) unless path.file?
    begin
      data = JSON.parse(path.read)
    rescue JSON::ParserError => e
      die("#{rs} is not a resultset (#{e.message}); re-run that project's test target.", 1)
    end
    unless record?(data)
      die("#{rs} is not a resultset of the shape shcov.rb run writes " \
          "({ name => { coverage => { path => { lines => [count or null...] } } } }); re-run that project's test target.", 1)
    end
    data.each_value do |run|
      run["coverage"].each do |rel, cov|
        cov["lines"].each_with_index { |n, i| merged[rel][i] = merged[rel][i].to_i + n if n }
      end
    end
  end

  rows = files.map do |rel|
    lines = merged.fetch(rel, []).dup
    Bashcov::Lexer.new(ROOT.join(rel).to_s, lines).complete_coverage
    relevant = lines.count { |n| !n.nil? }
    covered = lines.count { |n| n.to_i.positive? }
    [rel, covered, relevant, lines]
  end
  covered = rows.sum { |r| r[1] }
  relevant = rows.sum { |r| r[2] }
  pct = relevant.zero? ? 100.0 : covered * 100.0 / relevant

  merged_out = ROOT.join("target/shcov/merged.json")
  merged_out.dirname.mkpath
  merged_out.write(JSON.pretty_generate({ "merged" => { "coverage" => rows.to_h { |r| [r[0], { "lines" => r[3] }] }, "timestamp" => Time.now.to_i } }))

  summary = format("shcov: %.2f%% of shell lines covered (%d/%d, floor %d%%)", pct, covered, relevant, floor)
  if pct + 1e-9 < floor
    warn "shcov: per-script line coverage, least covered first:"
    rows.sort_by { |r| [r[2].zero? ? 100 : r[1] * 100.0 / r[2], r[0]] }.each do |rel, c, n, _|
      warn format("  %6.2f%%  %4d/%-4d  %s", n.zero? ? 100.0 : c * 100.0 / n, c, n, rel)
    end
    warn summary
    warn "shcov: below the floor — cover the missed lines with a journey that drives the real script (target/shcov/merged.json has the per-line hits)."
    exit 1
  end
  warn summary
end

case ARGV.shift
when "run"
  project, out, sep, *command = ARGV
  die("usage: shcov.rb run <project> <out.json> -- <command...>") unless sep == "--"
  run(project, out, command)
when "report"
  floor, *resultsets = ARGV
  report(floor.to_s, resultsets)
else
  die("usage: shcov.rb run <project> <out.json> -- <command...> | report <floor> <resultset.json...>")
end
