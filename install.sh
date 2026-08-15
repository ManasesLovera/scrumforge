#!/usr/bin/env bash
# scrumforge installer: build release binary, symlink into ~/.local/bin,
# and register the `scrumforge` alias in bash and zsh rc files.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
TARGET="$BIN_DIR/scrumforge"

echo "==> building scrumforge (release)…"
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
ln -sf "$PROJECT_DIR/target/release/scrumforge" "$TARGET"
echo "==> installed: $TARGET -> $PROJECT_DIR/target/release/scrumforge"

for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    [ -f "$rc" ] || continue
    if ! grep -q 'alias scrumforge=' "$rc"; then
        {
            echo ''
            echo '# scrumforge - AI scrum team orchestrator'
            echo 'alias scrumforge="'"$TARGET"'"'
        } >> "$rc"
        echo "==> alias added to $rc"
    else
        echo "==> alias already present in $rc"
    fi
done

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo ''
        echo "note: $BIN_DIR is not in your PATH. Add this line to your shell rc:"
        echo "  export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac

echo "==> done. Restart your shell or run: source ~/.zshrc"
