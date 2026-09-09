# Always-on controller

Install the Stackless CLI at `/usr/local/bin/stackless` on a Linux host. Install
Stripe CLI and Projects for the same account. Authenticate Stripe on that host.
The SSH account and service account must be the same user. Docker is required
only for container workloads on the controller host.

Install the user unit from this repository:

```sh
install -d -m 700 "$HOME/.config/systemd/user" "$HOME/.config/stackless"
install -m 600 deploy/systemd/stackless-controller.service "$HOME/.config/systemd/user/"
loginctl enable-linger "$USER"
systemctl --user daemon-reload
systemctl --user enable --now stackless-controller.service
stackless controller --json
```

Provider credentials and application secrets can be set in
`~/.config/stackless/controller.env`. Create that file with mode 0600. Restart the
unit after changing it. The file is read by systemd; it is not a shell script.
Source uploads exclude `.stackless.env`, `.env`, `.env.*`, and credential folders.

`stackless controller --json` checks that this process is the enabled unit's
`MainPID`, that `Restart=always` is set, that restart rate limiting is disabled,
and that user lingering is enabled. Any failed check reports `persistent: false`.
The lease reaper stays enabled in `--systemd-user` mode.

User lingering starts the user manager at boot and keeps it running after logout.
See the [systemd loginctl documentation](https://github.com/systemd/systemd/blob/main/man/loginctl.xml).
The restart behavior is defined by
[systemd.service](https://github.com/systemd/systemd/blob/main/man/systemd.service.xml).

The unit uses `KillMode=process` so controller restart does not kill journaled
workloads. Run `stackless down` to remove those workloads. Stopping the unit stops
lease enforcement until it is started again. Keep the state directory on durable
storage. Do not run another controller against the same state file.

Configure an OpenSSH host alias and verify the host key through your normal SSH
setup. Then connect from your workstation:

```sh
stackless --controller ssh://builder controller --json
stackless --controller ssh://builder up --file stackless.toml --on render --no-wait --json
stackless --controller ssh://builder operation get OPERATION_ID --json
```

`STACKLESS_CONTROLLER=ssh://builder` selects the same controller for CLI, SDK, and
MCP calls. SSH runs with batch authentication and strict host-key checking.
The bridge invokes `stackless daemon remote-control` through the remote account's
PATH. It does not start an unsupervised controller when the service is absent.

Source uploads and definitions are persisted with the operation before extraction.
Each request allows 64 MiB of source contents and 100000 entries. Symlinks, special
files, and traversal paths are rejected. Uploaded source trees become immutable
inputs; execution uses owned snapshots. Use Git sources for larger trees.

This unit has not yet been deployed or verified on a Linux host during the
overhaul. The local bridge, upload, controller restart, and teardown tests pass.
