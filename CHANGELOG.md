# Changelog

What changed in each release of Pathfinder, newest first. `pathfinder update` shows the newest section.

## 0.3.3

- **Updates show up within a minute.** `pathfinder update` and the installer no longer see an old release list that Cloudflare kept too long; while the server is down they still fall back to the cached copy.
- **The program is now `pathfinder`, not `harness`.** Installs that were updated in place under the old name move to `pathfinder` the first time they run, and `harness` stays as a link to it, so a Zed setting that runs `harness` keeps working. Change it to `pathfinder` when you like.

## 0.3.2

- **The commands you'd expect.** `/quit` (or `/exit`, `/q`), `/help`, `/new` (or `/clear`), `/resume`, `/model`, `/status`, `/copy`, `/thinking`, `/join`, `/data` and `/update`. Typing `/` shows the commands that match; Tab completes. An unknown command says so instead of going to the model.
- **The input edits like a shell:** Ctrl+A, Ctrl+U, Ctrl+K, Ctrl+W, Alt+Backspace, and Ctrl+←/→ to move by a word. Ctrl+letters without a meaning no longer type the letter.
- `/help` shows every key and command, and scrolls.

## 0.3.1

- **Updates stay consistent while caches catch up.** Each release now has its own signed manifest that never changes, and `pathfinder update` checks that one, so a cache that hasn't caught up can only offer an older release, not a broken one.
- **Plainer prompts.** The answer and test prompts no longer describe the user as a student; they work the same for any kind of work.
- A new icon.

## 0.3.0

The command is now `pathfinder`, and this release gets Pathfinder ready for a wider beta.

- **New name on the command line.** Run `pathfinder` (no `tui` needed). Installs from before 0.3 keep working: `harness` still runs, and `pathfinder` is added beside it. Environment variables are now `PATHFINDER_*`; the `HARNESS_*` names still work.
- **Bug reports.** `/bug what happened` (or `pathfinder bug`) builds a report with your settings, the last log lines and the last crash, shows you all of it, and sends it only when you confirm. Without joining the beta it is saved to a file instead.
- **Crashes.** A crash puts your terminal back as it was and writes the details to `crash.log` on your computer. A crash inside one reply ends that reply, not the app.
- **The shared gate is used for the gate only**, and only when you have no decision key of your own. Before, other decisions (reading what a message meant, tagging the agent's changes) could go through it too. Your own keys now always come first.
- **When the gate can't check a reply**, the held-back reply now says why: today's shared checks are used up, or the service couldn't be reached.
- **The welcome screen** separates joining the beta from sharing data, and says what declining means.
- **Privacy notice version 3** covers bug reports, crashes, update results and first-time milestones in usage events, and the download counts the server keeps. If you share data, Pathfinder asks again.
- **Clearer errors.** Network problems are one plain sentence; `PATHFINDER_DEBUG=1` shows the details. `pathfinder help` leads with what you need first. Unknown commands exit with an error.
- **`pathfinder paths`** also shows the log and the crash log.
- Logs no longer contain held-back drafts or full error responses from model providers.

## 0.2.1

- Pathfinder finds its folders the way your system expects: `HARNESS_HOME`, the XDG variables, folders from earlier versions, then the platform's own place.
- The installer honours `PATHFINDER_BIN_DIR` and `XDG_BIN_HOME`, and prints the PATH line for your shell.
- `/share` shortens your home folder in shared threads even when `HOME` isn't set.

## 0.2.0

- Join the beta by signing in with your email in the browser, instead of typing an invite code (codes still work).
- Privacy notice version 2: your email is stored with what you send.

## 0.1.0

- The first signed and notarised release for macOS, with `harness update`.
- Optional usage events and `/share`, with the consent screen and `harness data`.
