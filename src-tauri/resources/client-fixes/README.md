# Bundled Client fixes PAK

These compiled assets translated the cheat menu successfully in the supported 1.3.0.473797 client. They contain 69 translated descriptions and two parameter hints. Both package indexes and payloads use the build's custom encryption.

SHA256:
- BravoHotelGame-ClientFixes_P.pak: 002174c2509a48a3bcb7beb56dbae35aae6db00fd48eb402e9d0543e1a38c506
- BravoHotelGame-ClientFixes_P.sig: 4a3a1c4a79b06aebd5f42678603f9fa9d52b66c2c5ae7d50816ae7e5027103e0

Rebuild using the tools in SDK and Research/pak-fixes/ClientFixes, verify the translated exports offline, sign with the external private key, and replace both files together. Commit only compiled assets and public verification material. The workflow verifies the actual bundled signature before embedding the resources in the launcher.
