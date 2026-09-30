# PawnIO AMDFamily17 module 0.2.2

Unmodified signed module: monitor-app/src-tauri/resources/pawnio/AMDFamily17.bin.
SHA-256: 099dc01d6db97ea997fec4a461e191cc64b9d7ce47c9d2153c451c56c2adcf50.
Official release: https://github.com/namazso/PawnIO.Modules/releases/tag/0.2.2
Source commit: e12a858d952461ee2e919897cacbff7f905fe370.

AMDFamily17.p and COPYING retain the LGPL-2.1-or-later license. The included
headers retain their own 0BSD notices. Upstream build instructions and the
full source are available in the release/tag above.

The app is an independent device-IOCTL client. It invokes only ioctl_read_smn
at fixed offset 0x59800; it exposes no write-MSR or caller-selected commands.
The stock module contains other entry points which this app does not call.

For development or relinking, replace the embedded module at the stated path,
update its expected digest in enhanced/cpu_read.rs, and rebuild the Rust app
or probe. Modified modules must still satisfy the installed driver's signature
policy; no security setting or unrestricted driver is configured by the app.
The main application's license is not selected by this vendor notice.
