# Operating system integration

The core requests a semantic operation from a platform adapter. It never builds a command by concatenating model output. Each adapter reports availability, required OS authorization, permitted resource forms and an explicit failure reason. UI clients use the same service contracts as the CLI.

## Scoped file reads

1. The user selects a local directory through a trusted client. Open it as an OS handle and bind its identity to a session-scoped root ID. Deny devices, network shares and unqualified filesystems in the first release.
2. Accept a bounded sequence of relative path components. Reject parent traversal, absolute paths, empty components and embedded NULs. On Windows also reject UNC/device namespaces, alternate data streams, reserved device names and ambiguous trailing dots/spaces.
3. Resolve components through the held directory, without following links or crossing mount/reparse boundaries. Pin opened ancestors through the operation. Never validate a canonicalized string and later reopen the original name. Fail closed on a platform where these guarantees cannot be demonstrated.
4. Open the final file read-only and inspect that same handle for a regular file, identity and size. Refuse FIFOs, sockets, devices and multiply linked files for the initial strict profile. Read at most the configured byte limit with a deadline. Size changes do not defeat the read cap. Return a digest of the actual bytes consumed and indicate partial reads.
5. Recheck the grant epoch before dispatch and before releasing data. Audit the decision, safe relative resource identifier and byte count. A revoked or expired grant suppresses result delivery.

Linux can use a root directory FD with `openat2`, `RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS` and `RESOLVE_NO_XDEV`. Use the kernel's actual result and fail closed on unsupported flags. On macOS qualify an fd-relative no-follow component walk, device-boundary checks and rename-race semantics on the supported filesystem. For Windows qualify handle-relative opens with reparse-point rejection throughout the path and volume/file identity checks. Evaluate cap-std as a portability layer and test its actual behavior against the stricter product profile. Disable scoped reads on targets where adversarial rename races remain unresolved. [Linux openat2](https://man7.org/linux/man-pages/man2/openat2.2.html) · [cap-std directory handles](https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html) · [Windows reparse points](https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points)

## Adapter inventory

| Area | Windows | Linux | macOS |
| --- | --- | --- | --- |
| Discovery | Win32 topology/memory, DXGI adapters. WMI enriches inventory off the hot path. ETW and performance counters are opt-in diagnostics. | Read permitted procfs/sysfs counters, udev inventory and cgroup limits. A container may report smaller budgets than its host. | System frameworks and sysctl for memory/CPU, Metal device discovery. IOKit sensor visibility varies. |
| Process tree | Spawn an absolute bundled executable with explicit handle list and fresh environment. Evaluate restricted token/AppContainer. Job objects bound lifecycle and resources. | Spawn exact executable with close-on-exec descriptors, disposable working directory and process group. Qualify cgroups, namespaces and seccomp as available. | Spawn signed helper, explicit descriptors and process group. Qualify entitlements and sandbox profile for distribution method. |
| User automation, future | UI Automation for accessible controls. Distinct grants for screen capture, input and process launching. PowerShell requires a separately supervised command facility. | Prefer desktop portals for consent-mediated screen/input on Wayland. X11 access is broad and cannot provide equivalent isolation. D-Bus and systemd are optional. | Accessibility and Apple Events for supported apps, Core Graphics for allowed display operations. TCC denial returns a permission-needed result. |
| Services and installs, future | Explicit service name and verb through SCM. No arbitrary elevated PowerShell. | Allowlisted systemd unit over D-Bus where available, separate authorization through polkit. | Allowlisted service operations through a signed helper subject to OS authorization. |

These are candidate integrations. Job objects control groups of processes. They do not independently restrict filesystem or network access. Desktop portals depend on the compositor and backend. App Sandbox entitlements and privacy-consent prompts cover different resources. [Windows job objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) · [XDG RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html) · [Apple App Sandbox](https://developer.apple.com/documentation/security/app-sandbox)

## Privileged broker design

Defer the broker entirely until a named use case needs it. The future installation path registers an independently signed helper with a small, versioned set of administrative verbs. The OS authenticates the calling principal and process over an ACL-protected channel. Bind a challenge, nonce, expiry, request digest, allowed resource and operation to the current session. The helper independently checks allowlists, OS elevation approval, revocation and replay. A single-use permit covers only a specific typed action, for example restarting an allowlisted service. No path to execute arbitrary images, scripts, firmware operations or kernel writes.

A helper records intent before acting and an outcome or indeterminate status afterwards. Return actionable errors such as `unsupported`, `permission_required`, `scope_changed`, `resource_busy` and `unknown_outcome`. Never repeat a service operation after disconnection until reconciliation by resource identity. Host diagnostics remain read-only even when an administrative helper is installed.

## Qualification

Run tests on real filesystems for case folding, Unicode aliasing, symlink and mount escapes, Windows junctions, hard links, races during parent rename, revoked scope, file growth, concurrent cancellation and worker descendants surviving supervisor exit. Include headless Linux, a denied macOS privacy prompt and non-admin Windows. Record the OS build, filesystem type, adapter version and explicit unsupported cases for every advertised capability.
