# Getting started with Canvasly

Canvasly keeps your designs, comments, and version history together in a
workspace. Everything you create lives inside one workspace, and a workspace
has its own members and billing plan.

## Create your first workspace

1. Sign in at `https://app.canvasly.example`.
2. Click **New workspace** in the left sidebar.
3. Enter a name, for example `Marketing site`, and click **Create**.

## Invite teammates

Open **Settings → Members** and enter each teammate's email address. Invited
teammates can see every design in the workspace.

## Connect the CLI

The CLI needs to know which project to push designs into. Link your checkout to
a project once:

```sh
canvasly link --project marketing-site
```

After that, `canvasly push` uploads to the linked project.

## Billing

Each workspace is billed separately. Change the plan under
**Settings → Billing**.
