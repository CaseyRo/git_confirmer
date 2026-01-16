# Git Confirmer

Git Confirmer is a fast, local TUI for scanning multiple git repositories and committing changes in batch. It uses ratatui for the interface and provides optional low-token OpenAI commit message generation.

## Features
- Scan one or more root folders for git repos.
- Compact status overview with selection and per-repo comments.
- Batch commit with a base message plus optional per-repo note.
- Optional OpenAI-generated commit message per repo (low token usage).
- Configurable theme colors via `theme.conf`.

## Install
Build once with Rust:

```bash
cargo build --release --manifest-path ./Cargo.toml
```

Install as a proper cargo binary (recommended):

```bash
cargo install --path .
```

Convenience wrapper:

```bash
./install.sh
```

Add a convenient alias in `~/.zshrc`:

```bash
alias gc='git_confirmer --root'
```

Then run:

```bash
gc .
```

## Usage
Default scan root is `~/dev` when no args are provided.

```bash
./git-confirmer.sh --root ~/dev --message "all changes in files"
```

Key bindings:
- `j/k` or arrows: move
- `space`: toggle selection
- `a`: select dirty repos
- `n`: clear selection
- `e`: edit comment
- `g`: generate commit message (OpenAI)
- `c`: commit selected
- `r`: rescan
- `q`: quit

## Config
`config.toml` is loaded from the current directory (or set `GIT_CONFIRMER_CONFIG`).

```toml
# roots = ["~/dev", "~/work"]
# base_message = "all changes in files"
# theme_path = "./theme.conf"
```

## Theme
`theme.conf` is loaded from the current directory (or set `GIT_CONFIRMER_THEME`).
Edit the hex values to customize colors.

## OpenAI commit message generation
Set the API key and optional limits:

```bash
export OPENAI_API_KEY=...
export OPENAI_MODEL=gpt-4o-mini
export OPENAI_MAX_TOKENS=32
export OPENAI_TIMEOUT_SECS=10
```

Press `g` to generate messages for selected repos with changes.

## Open source projects used
- ratatui: https://github.com/ratatui/ratatui
- crossterm: https://github.com/crossterm-rs/crossterm
- walkdir: https://github.com/BurntSushi/walkdir
- dirs: https://github.com/soc/dirs-rs
- ureq: https://github.com/algesten/ureq
- serde: https://github.com/serde-rs/serde
- serde_json: https://github.com/serde-rs/json
- toml: https://github.com/toml-rs/toml

## Contributing
PRs and ideas are welcome. If you use this, share your workflow and improvements so we can make it better together.

## License
MIT. See `LICENSE`.
