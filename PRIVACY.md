# Privacy policy

**Last updated: 2 November 2026**

This policy covers Kode, published by K3D0 as **kodecraft** in the Microsoft Store and as **kode-rs** in the Snap Store and on GitHub.

**In short:** Kode doesn't collect your data. It has no accounts, analytics, advertising or crash reporting, and nothing you do in the app is sent to the publisher. Kode only connects to the network when you ask it to, and only to the services you choose.

## Information Kode handles

Kode is a code editor, so it works with whatever you open or enter. That can include personal information, depending on what you use it for:

- The files and folders you open, and your Git history, including the names and email addresses in commits.
- Your Git name and email, read from your Git settings to sign your commits.
- Database connection details and passwords, and the data your queries return.
- REST requests and responses, including any API keys, tokens and passwords you enter.

## How it's used and stored

Kode uses this information only to provide the feature you're using. Everything it saves stays on your computer:

| What | Where | Protection |
|---|---|---|
| Open files, cursor positions, undo history and unsaved changes | `.kode-rs.json` in your project folder | Not encrypted |
| REST collections and environments, including credentials you enter | `.kode-rs.json` in your project folder | Not encrypted |
| Code index | `.kode-rs` folder in your project | Not encrypted |
| Database connections | Kode's settings folder | Not encrypted |
| Database passwords | A separate file in Kode's settings folder | Not encrypted. On Linux and macOS, only your user account can read it |
| Diagnostic log, up to 4 MiB | Kode's settings folder | Not encrypted, and never sent anywhere |

Tokens from an OAuth sign-in are kept in memory only, and are lost when you close the app.

Anyone who can read these files can read what's in them, so treat them like the credentials they hold. **Don't commit or share `.kode-rs.json` or `.kode-rs.json.previous`**, for example by adding them to your project's `.gitignore`.

Kode's settings folder is `%APPDATA%\kode-rs` on Windows, `~/Library/Application Support/kode-rs` on macOS and `~/.config/kode-rs` on Linux, or the equivalent inside the app's private storage for Microsoft Store and Snap installs.

## Who receives your information

The publisher receives nothing. Kode only sends information when you use a feature that needs it, and only to these types of parties:

| When you | Kode connects to | What they receive |
|---|---|---|
| Send a REST request | The address you enter | Your request, including its credentials |
| Sign in with OAuth | The provider you configure | The sign-in details you enter |
| Connect to a database | The server you configure | Your credentials and queries |
| Install a database driver | Maven repositories, or the source you choose | Which driver you requested |
| Install a language server | GitHub, the npm registry, Eclipse, or the source its catalog lists | Which package you requested |
| Download the optional Java runtime | GitHub | Which download you requested |

These parties can also see your IP address, like any server you connect to, and their own privacy policies apply.

**Language servers** are separate programs. Kode sends them the files you open, so they can provide their features. Kode only starts servers that are already installed, and each server's own behaviour is up to its developer.

**Browser mode** (`kode serve`) shows the app to anyone who can reach its address. It only listens on your own computer unless you choose otherwise.

**The stores** that distribute Kode, Microsoft and Canonical, may collect their own data about downloads and installs under their own policies.

## Your choices

- **See your data:** everything Kode saves is in the files listed above, and you can open them in any text editor.
- **Delete your data:** delete those files, or remove a connection or request in the app. Deleting `.kode-rs.json` also discards any unsaved changes it holds.
- **Avoid the network:** every feature that uses the network is optional. Editing, search, Git and the built-in code intelligence all work offline.
- **Uninstall:** removing a Microsoft Store or Snap install also removes its private storage. The `.kode-rs.json` file and `.kode-rs` folder stay in your projects until you delete them.

## Changes to this policy

Changes are published here with a new date, and the [history of this file](https://github.com/alfu32/kodecraft/commits/work/PRIVACY.md) shows every earlier version.

## Contact

Please send privacy questions through [GitHub issues](https://github.com/alfu32/kodecraft/issues). Issues are public, so don't include personal information or credentials.
