# winget

Templates for the [winget-pkgs](https://github.com/microsoft/winget-pkgs) manifests of `amigo-labs.amigo-blinzel`.

After a release is published:

1. Fill `{{VERSION}}`, `{{SHA256_X64}}`, `{{SHA256_ARM64}}` (from the release's `SHA256SUMS`) and `{{LICENSE}}` (the license chosen for the repository).
2. Validate: `winget validate --manifest packaging/winget` and test locally: `winget install --manifest packaging/winget`.
3. Submit the three files to winget-pkgs under `manifests/a/amigo-labs/amigo-blinzel/<version>/`.

Later versions: `wingetcreate update amigo-labs.amigo-blinzel --version <v> --urls <x64-url> <arm64-url> --submit` updates the previous manifest automatically.
