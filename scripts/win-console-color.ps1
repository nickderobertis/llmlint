<#
.SYNOPSIS
  Verify that llmlint's colorized report actually RENDERS on a real Windows
  console - not just that it emits ANSI bytes.

.DESCRIPTION
  The hermetic e2e suite and the screenshot tooling both assert that llmlint
  *emits* ANSI escape codes; that is platform-independent and says nothing about
  whether a Windows console interprets them. On a legacy Windows console (no
  virtual-terminal processing) bare ANSI prints as `<-[31m` garbage. llmlint
  routes its report through anstream's `AutoStream`, which enables VT or
  translates the SGR codes into Win32 console attribute calls; this script proves
  that end result on a genuine console.

  It drives the REAL release binary against the mock-oneharness fixture
  (screenshots/fixture/) with `--color always` - no model, no network, no cost,
  deterministic - into a freshly created console screen buffer, waits for it to
  exit, then reads the buffer back cell-by-cell with `ReadConsoleOutput`. The
  signal is the per-cell *attributes*: the `FAIL` label must carry the red
  foreground, `PASS` the green one, and no cell may contain a literal ESC (0x1b).
  A pre-fix build (bare ANSI to a fresh buffer, VT off) leaves raw ESC bytes in
  the cells and fails here; the AutoStream build renders real color and passes.

  Run from the repo root. Exits 0 on success, 1 on a rendering assertion
  failure, and throws (non-zero) on a setup/Win32 error - never a silent skip.
#>
[CmdletBinding()]
param(
    [string]$Llmlint = "target/release/llmlint.exe",
    [string]$MockOneharness = "target/release/llmlint-mock-oneharness.exe",
    [string]$Fixture = "screenshots/fixture"
)

$ErrorActionPreference = "Stop"

# Resolve to absolute paths now: the child runs with its working directory set to
# the fixture, so relative paths would otherwise break.
$Llmlint = (Resolve-Path -LiteralPath $Llmlint).Path
$MockOneharness = (Resolve-Path -LiteralPath $MockOneharness).Path
$Fixture = (Resolve-Path -LiteralPath $Fixture).Path
$Config = Join-Path $Fixture "llmlint.yml"
$Verdicts = Join-Path $Fixture "verdicts.json"

$cs = @"
using System;
using System.Runtime.InteropServices;

public static class Con {
    public const int STD_OUTPUT_HANDLE = -11;
    public const uint GENERIC_READ = 0x80000000;
    public const uint GENERIC_WRITE = 0x40000000;
    public const uint FILE_SHARE_READ = 0x1;
    public const uint FILE_SHARE_WRITE = 0x2;
    public const uint CONSOLE_TEXTMODE_BUFFER = 1;
    public const uint HANDLE_FLAG_INHERIT = 1;

    [StructLayout(LayoutKind.Sequential)]
    public struct COORD { public short X; public short Y; public COORD(short x, short y){X=x;Y=y;} }

    [StructLayout(LayoutKind.Sequential)]
    public struct SMALL_RECT { public short Left, Top, Right, Bottom; }

    // CHAR_INFO is a union of a UTF-16 char and the cell attributes; read with
    // ReadConsoleOutputW so the char field is wide.
    [StructLayout(LayoutKind.Explicit)]
    public struct CHAR_INFO { [FieldOffset(0)] public char Char; [FieldOffset(2)] public ushort Attributes; }

    [StructLayout(LayoutKind.Sequential)]
    public struct CONSOLE_SCREEN_BUFFER_INFO {
        public COORD dwSize;
        public COORD dwCursorPosition;
        public ushort wAttributes;
        public SMALL_RECT srWindow;
        public COORD dwMaximumWindowSize;
    }

    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr CreateConsoleScreenBuffer(uint dwDesiredAccess, uint dwShareMode, IntPtr lpSecurityAttributes, uint dwFlags, IntPtr lpScreenBufferData);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetConsoleActiveScreenBuffer(IntPtr h);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetStdHandle(int nStdHandle, IntPtr h);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr GetStdHandle(int nStdHandle);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetConsoleScreenBufferSize(IntPtr h, COORD size);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetHandleInformation(IntPtr h, uint dwMask, uint dwFlags);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool AllocConsole();
    [DllImport("kernel32.dll")]
    public static extern IntPtr GetConsoleWindow();
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool ReadConsoleOutput(IntPtr hConsoleOutput, [Out] CHAR_INFO[] lpBuffer, COORD dwBufferSize, COORD dwBufferCoord, ref SMALL_RECT lpReadRegion);
}
"@
Add-Type -TypeDefinition $cs

function Win32Msg() {
    return ([System.ComponentModel.Win32Exception]::new([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())).Message
}

# A console must exist before we can create a screen buffer. GitHub Actions steps
# already run under a console host; AllocConsole is the fallback for any host that
# doesn't (it fails harmlessly when one is already attached).
if ([Con]::GetConsoleWindow() -eq [IntPtr]::Zero) { [void][Con]::AllocConsole() }

$invalid = [IntPtr]::new(-1)
$buf = [Con]::CreateConsoleScreenBuffer(
    ([Con]::GENERIC_READ -bor [Con]::GENERIC_WRITE),
    ([Con]::FILE_SHARE_READ -bor [Con]::FILE_SHARE_WRITE),
    [IntPtr]::Zero, [Con]::CONSOLE_TEXTMODE_BUFFER, [IntPtr]::Zero)
if ($buf -eq $invalid -or $buf -eq [IntPtr]::Zero) { throw "CreateConsoleScreenBuffer failed: $(Win32Msg)" }

# Make the buffer inheritable and point both the active screen buffer and the
# STD_OUTPUT handle at it, so the child's stdout lands here whichever way .NET
# wires the process up (console attach, or explicit std-handle inheritance when a
# stream is redirected).
[void][Con]::SetHandleInformation($buf, [Con]::HANDLE_FLAG_INHERIT, [Con]::HANDLE_FLAG_INHERIT)
[void][Con]::SetConsoleScreenBufferSize($buf, [Con+COORD]::new([int16]120, [int16]300))
[void][Con]::SetConsoleActiveScreenBuffer($buf)
$oldStdOut = [Con]::GetStdHandle([Con]::STD_OUTPUT_HANDLE)
[void][Con]::SetStdHandle([Con]::STD_OUTPUT_HANDLE, $buf)

$stderr = ""
$exit = -1
try {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Llmlint
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $false   # inherit our screen buffer as stdout
    $psi.RedirectStandardError = $true     # keep the -v trace out of the buffer
    $psi.WorkingDirectory = $Fixture
    # `-v` so the PASS label is itemized (green) alongside the FAIL label (red);
    # `--color always` forces color regardless of TTY; `--max-parallel 1` keeps
    # the multi-judge lines ordered. Mirrors scripts/screenshots.sh.
    $psi.Arguments = "-c `"$Config`" --oneharness-bin `"$MockOneharness`" --color always --max-parallel 1 -v"
    $psi.EnvironmentVariables["LLMLINT_MOCK_VERDICTS"] = $Verdicts
    $psi.EnvironmentVariables["LLMLINT_MOCK_STATE"] = (Join-Path $env:TEMP ("llmlint-winstate-" + [guid]::NewGuid().ToString()))
    # Results logging is on by default; disable it so this render is side-effect-
    # free (no record written to the real user data dir) and the note never lands
    # in the console buffer we assert on.
    $psi.EnvironmentVariables["LLMLINT_NO_HISTORY"] = "1"

    $p = [System.Diagnostics.Process]::Start($psi)
    $stderr = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    $exit = $p.ExitCode
}
finally {
    # Restore the console before reading back, regardless of what happened.
    [void][Con]::SetStdHandle([Con]::STD_OUTPUT_HANDLE, $oldStdOut)
    [void][Con]::SetConsoleActiveScreenBuffer($oldStdOut)
}

# Read back the rendered cells. Use the buffer's real width; cap the rows we pull
# (the report is short) so a single ReadConsoleOutput stays well under its limit.
$info = New-Object Con+CONSOLE_SCREEN_BUFFER_INFO
if (-not [Con]::GetConsoleScreenBufferInfo($buf, [ref]$info)) { throw "GetConsoleScreenBufferInfo failed: $(Win32Msg)" }
$width = [int]$info.dwSize.X
$rows = [Math]::Min([int]$info.dwSize.Y, 60)

$cells = New-Object 'Con+CHAR_INFO[]' ($width * $rows)
$region = New-Object Con+SMALL_RECT
$region.Left = 0; $region.Top = 0; $region.Right = [int16]($width - 1); $region.Bottom = [int16]($rows - 1)
$ok = [Con]::ReadConsoleOutput($buf, $cells, [Con+COORD]::new([int16]$width, [int16]$rows), [Con+COORD]::new([int16]0, [int16]0), [ref]$region)
if (-not $ok) { throw "ReadConsoleOutput failed: $(Win32Msg)" }

# Win32 console foreground attribute bits.
$RED = 0x4; $GREEN = 0x2; $BLUE = 0x1

# Reconstruct the text grid and watch for any raw ESC byte (proof that ANSI was
# NOT interpreted - it would render as visible escape garbage).
$lines = New-Object System.Collections.Generic.List[string]
$escFound = $false
for ($y = 0; $y -lt $rows; $y++) {
    $sb = New-Object System.Text.StringBuilder
    for ($x = 0; $x -lt $width; $x++) {
        $ch = $cells[$y * $width + $x].Char
        if ([int]$ch -eq 27) { $escFound = $true }
        [void]$sb.Append($ch)
    }
    $lines.Add($sb.ToString().TrimEnd())
}

# A label is "rendered <color>" when every one of its cells has the required
# foreground bit set and the other two color bits clear (bold may add intensity,
# which we don't forbid). Returns the first matching row, else $false.
function Test-LabelColor([string]$label, [int]$needBit, [int]$forbidBits) {
    for ($y = 0; $y -lt $rows; $y++) {
        $idx = $lines[$y].IndexOf($label)
        if ($idx -lt 0) { continue }
        $good = $true
        for ($k = 0; $k -lt $label.Length; $k++) {
            $a = [int]$cells[$y * $width + ($idx + $k)].Attributes
            if ((($a -band $needBit) -eq 0) -or (($a -band $forbidBits) -ne 0)) { $good = $false; break }
        }
        if ($good) { return $true }
    }
    return $false
}

$failRed = Test-LabelColor "FAIL" $RED ($GREEN -bor $BLUE)
$passGreen = Test-LabelColor "PASS" $GREEN ($RED -bor $BLUE)

Write-Host "llmlint exit: $exit"
Write-Host "rendered console buffer (first non-empty lines):"
$lines | Where-Object { $_ -ne "" } | Select-Object -First 12 | ForEach-Object { Write-Host "  $_" }

$failed = $false
if ($escFound) {
    Write-Host "ASSERT FAIL: a raw ESC (0x1b) is present in the console buffer - ANSI was NOT interpreted; colors would render as escape garbage."
    $failed = $true
}
if (-not $failRed) {
    Write-Host "ASSERT FAIL: the 'FAIL' label is not rendered with a red foreground in the console buffer."
    $failed = $true
}
if (-not $passGreen) {
    Write-Host "ASSERT FAIL: the 'PASS' label is not rendered with a green foreground in the console buffer."
    $failed = $true
}

if ($failed) {
    Write-Host "--- llmlint stderr ---"
    Write-Host $stderr
    exit 1
}

Write-Host "OK: the Windows console rendered FAIL red and PASS green with no raw escapes - terminal coloring works on a real Windows console."
exit 0
