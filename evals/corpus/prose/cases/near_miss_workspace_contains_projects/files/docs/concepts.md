# Canvasly concepts

Canvasly has two levels of organization, and they are different things.

- A **workspace** is your team's account. It has members and a billing plan.
- A **project** is a folder of related designs inside a workspace. A workspace
  can hold any number of projects.

## Members

Members belong to the workspace, not to individual projects: anyone you invite
to the workspace can open every project in it.

## Billing

Billing is per workspace. Creating more projects never changes the price.

## Connect the CLI

The CLI pushes designs into one project. Link your checkout to a project once:

```sh
canvasly link --project marketing-site
```

The CLI finds the workspace from your login, so you only name the project.
