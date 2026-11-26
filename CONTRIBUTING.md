# Contributing

Thanks for wanting to help. Bug reports, ideas and small fixes are all welcome.

## Reporting bugs

`/bug what happened` in Pathfinder (or `pathfinder bug "what happened"`) collects what helps and shows it to you first. Or open an issue with the bug template.

## Building and testing

You need a recent stable Rust toolchain. There are three crates, each built on its own:

```sh
(cd app && cargo test)
(cd schema && cargo test)
(cd collector && cargo test)
```

Before sending a change, run in each crate you touched:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same on macOS and Linux.

`pathfinder eval` runs the live evals against your configured models (see [docs/reference.md](docs/reference.md#evals)). Run the `gate` suite when you change anything the gate depends on.

## How the code is meant to be

- [docs/theory.md](docs/theory.md) is the design. A change in behaviour starts there; code comments point to its sections (`§5`, `A6`).
- Text the user reads is plain and short: say what happened and what to do, no hype.
- Comments say why, not what. No banners, no commented-out code.
- Usage events may only hold numbers, flags and closed sets of names (see `schema/`). Adding an event means a new privacy notice version.
- Name the constants: timeouts, limits and addresses live in one place.

## Commits

One summary line, in plain English, saying what the change does.

## License

Pathfinder is licensed under the AGPL-3.0-or-later. By sending a change you agree that it is licensed under the same terms. If a change brings in code from elsewhere, say where it comes from and under which licence.

## Conduct

Be kind and assume good faith. Harassment and personal attacks aren't welcome here; the maintainer may remove comments and block people who don't respect that. Concerns go to alper@saritas.io.
