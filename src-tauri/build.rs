fn main() {
    // The no-Steam DLL is embedded by src/shim.rs with include_bytes!, which
    // fails the build outright if the file is missing. A checkout without the
    // binary -- a fresh clone, or a machine that has not built it yet -- should
    // still compile, so the include is behind a cfg set here.
    println!("cargo:rustc-check-cfg=cfg(has_shim)");
    println!("cargo:rustc-check-cfg=cfg(has_client_fixes)");
    // Same pattern for 7-Zip's standalone extractor, which only the Download
    // tab's backup uses (download.rs). Without it the backup needs 7-Zip
    // installed on the player's PC.
    println!("cargo:rustc-check-cfg=cfg(has_7zr)");
    println!("cargo:rerun-if-changed=resources/7zr.exe");
    if std::path::Path::new("resources/7zr.exe").is_file() {
        println!("cargo:rustc-cfg=has_7zr");
    } else {
        println!("cargo:warning=resources/7zr.exe is missing -- the Download tab's backup will need 7-Zip installed on the player's PC. See resources/README.md.");
    }
    println!("cargo:rerun-if-changed=resources/XAPOFX1_5.dll");
    if std::path::Path::new("resources/XAPOFX1_5.dll").is_file() {
        println!("cargo:rustc-cfg=has_shim");
    } else {
        println!("cargo:warning=resources/XAPOFX1_5.dll is missing -- the launcher will NOT install the no-Steam DLL. Get it from the latest sp-native release with tools/fetch-binaries.ps1 before shipping.");
    }
    println!("cargo:rerun-if-changed=resources/SPClientFixes.dll");
    println!("cargo:rerun-if-changed=resources/client-fixes/BravoHotelGame-ClientFixes_P.pak");
    println!("cargo:rerun-if-changed=resources/client-fixes/BravoHotelGame-ClientFixes_P.sig");
    if ["resources/SPClientFixes.dll", "resources/client-fixes/BravoHotelGame-ClientFixes_P.pak",
        "resources/client-fixes/BravoHotelGame-ClientFixes_P.sig"].iter().all(|p| std::path::Path::new(p).is_file()) {
        println!("cargo:rustc-cfg=has_client_fixes");
    } else {
        println!("cargo:warning=Client fixes resources are incomplete -- the optional setting requires the DLL, PAK and signature. Get them with tools/fetch-binaries.ps1.");
    }

    #[cfg(target_os = "windows")]
    {
        // Runs as a normal user (asInvoker) since 0.3.0. Requiring admin at
        // every start stopped the launcher from opening at all on some PCs.
        // The one thing that needs admin -- the hosts entries -- is done by an
        // elevated helper copy of the exe when needed (see hosts.rs).
        //
        // `app_manifest` REPLACES tauri's default manifest wholesale, not
        // merges with it — and the default one exists solely to declare a
        // dependency on comctl32.dll v6 (the "Common-Controls" assembly).
        // Dropping that dependency, as an earlier version of this manifest
        // did, makes Windows fall back to the ancient v5 comctl32.dll, which
        // is missing exports like TaskDialogIndirect that Tauri/WebView2
        // rely on — and the app then fails to start at all with
        // STATUS_ENTRYPOINT_NOT_FOUND. So both pieces have to live in the
        // one manifest below.
        let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
        let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
        let attrs = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attrs).expect("failed to run tauri-build");
    }
    #[cfg(not(target_os = "windows"))]
    {
        tauri_build::build();
    }
}
