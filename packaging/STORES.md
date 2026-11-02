# Publishing to the stores

Every run of the [Release workflow](../.github/workflows/distribution.yaml) builds the packages both stores need. Download them from the [release](https://github.com/alfu32/kodecraft/releases), then follow the steps below.

## Microsoft Store

The app is **kodecraft** (Store ID `9NG04V0XN51S`). Its identity comes from [`msstore-info.txt`](msstore-info.txt), and every build is installed and checked against it, including the `koders`, `kode` and `kd` commands.

**Package version:** the Store rejects versions starting with 0, so packages add 1 to the major version. Release `0.2.10` is package version `1.2.10.0`. Inside the app, the version still reads `0.2.10`.

1. In [Partner Center](https://partner.microsoft.com/dashboard), open **kodecraft** and start a submission.
2. **Packages:** upload `kode-<version>-windows-x64.msix` and `kode-<version>-windows-arm64.msix`. They're unsigned on purpose, because the Store signs them.
3. **Store listings:** choose **Import listings**, then **Import folder**, and select [`msstore`](msstore). Keep the folder's name, because the listing's image paths start with it. The text fills every language in the listing, so remove any language you don't want to offer in English.
4. **Properties:** enter the values below.
5. **Age ratings:** complete the questionnaire. The app has no user-to-user communication, purchases or location use, and can reach the internet only when you ask it to.
6. **Pricing and availability:** choose these yourself.
7. **Submission options:** paste the two notes below.

| Property | Value |
|---|---|
| Category | Developer tools, Development kits or IDEs if offered |
| Website | `https://github.com/alfu32/kodecraft` |
| Support contact | `https://github.com/alfu32/kodecraft/issues` |
| Privacy policy | `https://github.com/alfu32/kodecraft/blob/work/PRIVACY.md` |

**Why the app needs `runFullTrust`:**

> kodecraft is a terminal code editor. It runs as a console application and needs full trust to edit files the user chooses anywhere on disk, start the user's shell, and start language servers and a database helper as child processes.

**Notes for certification:**

> kodecraft is a keyboard-driven code editor that runs inside a console window, not a graphical window.
>
> To test it, start kodecraft from the Start menu. It opens your user folder. Or open Windows Terminal and run "kode", "koders" or "kd", optionally followed by a folder or file.
>
> Select a file in the tree on the left to open it. Press Alt+1 to Alt+8 to switch panels, Alt+8 for Help, and Ctrl+Q to quit.
>
> No account or test data is needed. Database connections need Java 21 or newer and a database to connect to, and the REST panel needs a server to call. All other features work offline.

**Updating the listing:** edit the `default` column of the CSV in `msstore`, then import the folder again. Up to 10 desktop screenshots are allowed, as PNG files of at least 1366 x 768. Leave "What's new" blank for the first submission. After the first import, Partner Center's exported CSV replaces image paths with URLs, so keep this copy as the source.

## Snap Store

The snap is [**kode-rs**](https://snapcraft.io/kode-rs). It can be released to every channel: edge, beta, candidate and stable. Its name, summary, description, links and icon are defined in [`package-snap.py`](../scripts/package-snap.py).

Upload both architectures and refresh the listing. Each upload prints its revision number:

```sh
snapcraft upload kode-rs_<version>_amd64.snap --release=edge
snapcraft upload kode-rs_<version>_arm64.snap --release=edge
snapcraft upload-metadata kode-rs_<version>_amd64.snap
```

After testing with `sudo snap install kode-rs --edge`, promote each revision:

```sh
snapcraft release kode-rs <revision> candidate
snapcraft release kode-rs <revision> stable
```

The Snap Store dashboard's [listing page](https://snapcraft.io/kode-rs/listing) is the only place to add screenshots, the category and a license display. Use the images in [`msstore/screenshots`](msstore/screenshots).
