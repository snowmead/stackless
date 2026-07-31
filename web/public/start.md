# Skill: Install stackless and add the agent skill

You are helping the user install the stackless CLI and add its agent skill so they can run ephemeral software stacks end to end.

## Step 1: Gather context

Fetch and read:

- https://stackless.sh/llms.txt
- https://github.com/snowmead/stackless/blob/main/.cursor/skills/stackless/SKILL.md

## Step 2: Install the CLI

Run the release installer (preferred over building from source):

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh
stackless --version
```

Expect `0.2.2` or newer. If `stackless` is not on `PATH` after install, refresh the shell environment and retry `--version` before continuing.

## Step 3: Add the agent skill

```bash
bunx skills add snowmead/stackless --skill stackless -g
```

This installs the stackless skill globally for the user's coding agents.

## Step 4: Confirm and hand off

1. Confirm `stackless --version` works.
2. Confirm the skill is available to the coding agent.
3. Tell the user they can paste a repo with `stackless.toml` (or run `stackless init` / `stackless adopt`) and drive the lifecycle with `--json`:

```bash
stackless check stackless.toml --on local --json
stackless doctor --file stackless.toml --json
stackless up --name demo --on local --json
stackless verify demo --json
stackless down demo --json
```

Always prefer `--json` and branch on `error.code`, never prose. Read the skill and schema before improvising flags.
