# collector

The service behind Pathfinder's beta. It:

- lets invited people join, by email sign-in (behind Cloudflare Access) or an invite code;
- receives usage events, shared threads and bug reports from installs that agreed to send them;
- forwards gate checks to TypeSafe with the operator's key, within daily budgets (the shared gate);
- serves the installer, the release manifest and releases, and the public pages.

The maintainer runs one instance at https://pathfinder.lprsrts.com. You can run your own: build it, point the app at it with `PATHFINDER_COLLECTOR`, and set the operator details in `app/src/operator.rs`.

## Running it

```sh
cargo build --release
COLLECTOR_DB=./collector.sqlite COLLECTOR_PUBLIC=./public target/release/collector serve
```

The [Dockerfile](Dockerfile) builds an image from the repository root (`git archive HEAD schema collector | docker build -f collector/Dockerfile -`). It runs as uid 2002 and keeps its database in `/data`.

## Settings

All settings come from the environment.

| Variable | Default | What it sets |
|---|---|---|
| `COLLECTOR_ADDR` | `0.0.0.0:8080` | Where it listens |
| `COLLECTOR_DB` | `/data/collector.sqlite` | The SQLite database |
| `COLLECTOR_PUBLIC` | `/public` | The folder served for every path the API doesn't handle |
| `COLLECTOR_PUBLIC_URL` | `http://127.0.0.1:8080` | Its public address; the sign-in form must come from it |
| `COLLECTOR_OPERATOR` | `the person who invited you` | Who runs it, as people know them ("Ask … to add you") |
| `COLLECTOR_COMMIT` | `unknown` | The source commit, reported on `/v1/version` (the image sets it) |
| `TYPESAFE_KEY_FILE` | none | A file holding the TypeSafe key for the shared gate; without it the gate answers 503 |
| `TYPESAFE_BASE_URL` | `https://api.typesafe.ai/v1` | TypeSafe's API |
| `GATE_MODEL` | `jev-1.14.0` | The only model the shared gate forwards to |
| `GATE_DAILY_CAP` | `400` | Gate checks per person per day, across all their installs |
| `GATE_GLOBAL_DAILY_CAP` | `5000` | Gate checks per day for everyone |
| `GATE_TIMEOUT_SECS` | `8` | How long to wait for TypeSafe (the app waits 10) |
| `ACCESS_TEAM_DOMAIN` | none | The Cloudflare Access team, e.g. `team.cloudflareaccess.com`; with `ACCESS_AUD`, turns on email sign-in |
| `ACCESS_AUD` | none | The Access application's audience tag |
| `PATHFINDER_RELEASE_KEY` | none | For `sign`: the release key, instead of the macOS keychain |
| `RUST_LOG` | `info` | Log level |

Email sign-in expects a Cloudflare Access application in front of `/join` only, with the one-time PIN login. Access proves the address; the collector checks Access's signed token itself and lets in only addresses on its own list.

## Commands

| Command | What it does |
|---|---|
| `serve` | Run the service |
| `health` | Exit 0 if the service answers (for container health checks) |
| `allow <email>` | Let an address sign in |
| `disallow <email>` | Take an address off the list and stop its installs |
| `people` | Who may sign in |
| `invite <name>` | A single-use invite code, for someone without email sign-in |
| `stats` | One line per install: consent, counts, last seen |
| `report` | Active people, retention, reply times, gate and check outcomes, errors, crashes, versions, downloads |
| `reports [n]` | The newest bug reports |
| `gate-stats` | Gate checks per day, and per person today |
| `gate on\|off` | Resume or pause the shared gate, without a restart |
| `export [dir] [--with-emails]` | Events, threads, bug reports and installs as JSONL, under the data folder by default; people are pseudonyms unless `--with-emails` |
| `stop <name\|install>` | Stop collecting from someone (their app stops on its next upload) |
| `prune` | Delete events past retention (also runs daily) |
| `backup <file>` | A consistent copy of the database, safe while it runs |
| `alerts` | Problems worth a notification, one per line (the gate paused, 80% of its daily budget used, many TypeSafe errors); nothing when all is well |
| `keygen`, `sign <file>` | Make the release signing key (macOS keychain) and sign a release manifest |

## API

Everything under `/v1` except signing in needs an install token (`Authorization: Bearer …`), checked before the request body is read.

| Route | What it does |
|---|---|
| `GET /healthz` | Liveness, including the database |
| `GET /v1/version` | Schema and notice versions, the commit and the source repository |
| `POST /v1/link`, `POST /v1/link/poll` | Browser sign-in: the app gets a code, the person approves it on `/join`, the app collects its token |
| `GET/POST /join` | The sign-in page (behind Cloudflare Access) |
| `POST /v1/enroll` | Join with an invite code |
| `POST /v1/events` | Usage events; unknown fields are refused one event at a time |
| `PUT /v1/consent` | A changed answer |
| `POST /v1/threads` | A shared thread |
| `POST /v1/reports` | A bug report |
| `DELETE /v1/me` | Erase everything from an install |
| `POST /v1/typesafe/systemone` | The shared gate |

Endpoints anyone can reach are rate-limited per address; addresses are kept in memory for a minute and never written down.
