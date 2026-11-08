# Secrets

All secrets used by the GitHub workflows, what they are for and what happens without them. They are set under **Settings → Secrets and variables → Actions**, either in this repository or, for organization secrets, in the `amigo-labs` organization. GitHub never shows a secret's value again after it is set: whoever creates one keeps the original safe (e.g. in a password manager).

| Secret | Where | Needed | Used by | Without it |
|---|---|---|---|---|
| [`RELEASE_SIGNING_KEY`](#release_signing_key) | repository | **yes** | `release.yml` | the release fails |
| [`MACOS_SIGNING_CERT`](#macos_signing_cert-and-macos_signing_cert_password) | organization | no | `release.yml` | the Mac app is signed ad hoc |
| [`MACOS_SIGNING_CERT_PASSWORD`](#macos_signing_cert-and-macos_signing_cert_password) | organization | with the certificate | `release.yml` | with the certificate set: the macOS job fails at the keychain import |
| [`RELEASE_PLEASE_TOKEN`](#release_please_token) | repository | no | `release-please.yml` | no CI runs on the release PR |

`GITHUB_TOKEN` (`github.token`) is not on this list: GitHub creates it for every run. The release workflow uses it to upload the files and publish the release.

`gh secret list` (repository) and `gh secret list --org amigo-labs` (organization, admin rights needed) show which secrets exist, without their values.

## `RELEASE_SIGNING_KEY`

**What it is:** the secret half of the project's own Ed25519 key, the 32-byte seed as hex.

**What it does:** the last job in `release.yml` signs `SHA256SUMS` with it and attaches the signature as `SHA256SUMS.sig`. The public half is built into the app (`RELEASE_KEY` in `crates/blinzel-core/src/update.rs`). The updater on every platform installs only a release whose `SHA256SUMS` carries a valid signature and whose file matches its checksum. This signature is the app's only proof of origin: Windows and macOS builds carry no code signing certificate from a CA.

**Creating it:**

```
cargo run -p blinzel-release -- keygen <file>
```

Store the file's content as the secret, delete the file and put the printed public key into `RELEASE_KEY`.

**Important:**
- If it is lost, installed copies accept no update anymore.
- A new key reaches installed copies only through a release that is still signed with the old one.
- Without the secret the release stops at "Sign SHA256SUMS" (`RELEASE_SIGNING_KEY secret is missing`) and nothing is published.
- It cannot sign the Mac app: `codesign` needs an X.509 certificate with an RSA or ECDSA key. Keep the two keys separate anyway: they protect different things.

## `MACOS_SIGNING_CERT` and `MACOS_SIGNING_CERT_PASSWORD`

**What they are:** a self-signed code signing certificate (CN `amigo-labs`, RSA 2048, valid for ten years) as a `.p12`, base64-encoded, and the `.p12`'s password. They are organization secrets, shared with the selected repositories, because one certificate serves all of amigo-labs' Mac apps.

**What they do:** the macOS job in `release.yml` imports the certificate into a temporary keychain on the runner and signs `amigo-blinzel.app` with it. Gatekeeper does not trust the certificate, so the warning at the first start stays. What it adds is a stable identity: macOS ties permissions such as Screen Recording (needed for screenshots) to the bundle id plus the certificate. An ad hoc signature changes with every build, so every update would lose those permissions.

**Creating them:** once, with

```
bash packaging/macos/create-signing-cert.sh
```

The script creates `~/amigo-labs-signing/signing.p12`, outside any repository, and sets both organization secrets through `gh`. That needs organization admin rights and the `admin:org` scope (`gh auth refresh -h github.com -s admin:org`). Without them, `--repo amigo-labs/amigo-blinzel` stores the two secrets in this repository instead. `--help` lists all options. Another repository gets access with `--repos amigo-blinzel,<other>` (the full list). Run the script with `bash`; do not `source` it.

**Important:**
- Do not replace the certificate: with a new one, every Mac asks for its permissions again.
- Keep `signing.p12` and its password safe. Neither can be read back from GitHub.
- Check after a release: in the macOS job, the step "Signing identity" must not report `MACOS_SIGNING_CERT is not set`, and the signature check shows `Authority=amigo-labs`.

## `RELEASE_PLEASE_TOKEN`

**What it is:** a fine-grained personal access token (or a GitHub App token) with write access to *contents* and *pull requests* of this repository.

**What it does:** release-please opens and updates the release PR with it, and the same workflow pushes the updated `Cargo.lock` to that PR. Changes made with the workflow's own `GITHUB_TOKEN` start no further workflows, so without this token CI does not run on the release PR. release-please still works then, with `github.token`.

**Important:**
- Either way, *Allow GitHub Actions to create and approve pull requests* must be on (Settings → Actions → General).
- A personal access token expires: renew it in time and store the new one as the secret.
