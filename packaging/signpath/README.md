# SignPath

Code signing through the [SignPath Foundation](https://signpath.org) (free for open source). The release workflow (`.github/workflows/release.yml`) submits each setup executable once `SIGNPATH_ORGANIZATION_ID` is set.

Names the workflow expects in SignPath:

| Setting | Value |
|---|---|
| Project slug | `amigo-blinzel` |
| Trusted build system | `GitHub.com`, linked to the project |
| Artifact configuration slug | `exe`, content: [`artifact-configuration.xml`](artifact-configuration.xml) |
| Signing policy slug | `release-signing`, manual approval |

Repository settings:

| Kind | Name | Value |
|---|---|---|
| Secret | `SIGNPATH_API_TOKEN` | API token of a SignPath user that may submit to `release-signing` |
| Variable | `SIGNPATH_ORGANIZATION_ID` | SignPath organization ID |
