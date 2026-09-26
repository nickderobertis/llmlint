# Contributing

Thanks for helping out. This page covers the branch workflow we use for every
change.

## Branch workflow

1. Create a branch from `main` with `git switch -c <your-name>/<topic> main`.
2. Commit your work in small commits with `git commit`.
3. Rebase onto the latest `main` with `git fetch origin && git rebase origin/main`.
4. Push the rebased branch with `git push --force origin main`.
5. Open a pull request with `gh pr create --fill`.

## Commit messages

Use Conventional Commits, for example `fix(parser): handle empty input`.
