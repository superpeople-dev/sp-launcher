# Client fixes DLL

**Apply client fixes** is enabled by default for the supported Super People preservation build. The launcher checks the executable SHA256 before enabling fixes and deploys the DLL with the bundled PAK/signature for one game session. Players can disable the option in Settings.

The client fixes debug window is off by default and can be enabled separately for the next launch. Diagnostics also go to an attached debugger when the window is hidden.

The DLL contains the First Blood audio reference fix, White/Gold capsule ID correction, local class-selection level adjustment and custom PAK support. The class adjustment's pre-match scope remains under investigation in the parent code folder's TODO.md. Its original level is retained for restoration.

Cheat-menu translations are in the PAK; there is no DLL translation implementation. Custom signatures use the committed RSA public key. Only the canonical ClientFixes PAK path accepts that key; original PAK validation and engine chunk checks remain in place. The signature covers the PAK digest and serialized chunk CRC table. Private signing material is never included.

The game's recent-PAK shortcut could select our .uasset and the original .uexp. The DLL validates the original seven-byte comparison before patching it to enforce ordered archive lookup. Existing threads are paused and checked while installing the patch. No cache polling or temporary lookup hooks remain. The optional console reports waiting/activation for each feature, successful custom signature verification, First Blood match transitions, and failures. Capsule and class activation and successful signature verification are reported once per process so polling does not flood the window. Signature verification reports archive trust, not proof that a particular translated asset was selected.

The [Build ClientFixes DLL](../.github/workflows/client-fixes-dll.yml) GitHub Action builds and uploads just `SPClientFixes.dll` for Windows x64. The separate unsigned launcher workflow runs CTest against synthetic valid/tampered signatures and the bundled PAK pair, checks deployment lifecycle behavior, and builds the launcher. PAK extraction, translation, packing and signing are separate from this DLL build.
