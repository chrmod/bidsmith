#!/usr/bin/env bash
# bidsmith bootstrap for an ephemeral Linux sandbox (e.g. Claude Cowork).
#
# Cowork wipes the sandbox home between sessions and does not carry PATH
# across bash calls, so install bidsmith into a MOUNTED folder and
# re-establish PATH every call. Copy this file into your mounted
# ~/.bidsmith/bin/bootstrap.sh once (~/.bidsmith persists — it's where
# bidsmith reads credentials.toml), then SOURCE it at the top of each
# bash call:
#
#   source ~/.bidsmith/bin/bootstrap.sh   # puts bidsmith on PATH
#
# It caches the Linux release binary beside itself and downloads only on
# a cache miss. Set BIDSMITH_VERSION to pin a release (default: latest);
# delete the cached binary to force a re-download. Safe to `source` — the
# download runs in a subshell, so it never leaks shell options into your
# session.

__bs_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
__bs_arch="$(uname -m)"
case "$__bs_arch" in
  aarch64 | arm64) __bs_tgt="aarch64-unknown-linux-gnu" ;;
  x86_64 | amd64) __bs_tgt="x86_64-unknown-linux-gnu" ;;
  *)
    echo "bidsmith bootstrap: unsupported sandbox arch '$__bs_arch'" >&2
    __bs_tgt=""
    ;;
esac
__bs_cached="$__bs_dir/bidsmith-linux-$__bs_arch"

if [ -n "$__bs_tgt" ] && [ ! -x "$__bs_cached" ]; then
  (
    set -euo pipefail
    if [ "${BIDSMITH_VERSION:-latest}" = "latest" ]; then
      base="https://github.com/chrmod/bidsmith/releases/latest/download"
    else
      base="https://github.com/chrmod/bidsmith/releases/download/${BIDSMITH_VERSION}"
    fi
    url="$base/bidsmith-${__bs_tgt}.tar.gz"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "$url" -o "$tmp/b.tgz"
    # github.com asset URLs work in the sandbox; api.github.com may be blocked.
    echo "$(curl -fsSL "$url.sha256" | awk '{print $1}')  $tmp/b.tgz" | sha256sum -c -
    tar xzf "$tmp/b.tgz" -C "$tmp"
    install -m 0755 "$tmp/bidsmith" "$__bs_cached"
  ) || echo "bidsmith bootstrap: install failed (check the network allow list)" >&2
fi

if [ -x "$__bs_cached" ]; then
  ln -sf "$__bs_cached" "$__bs_dir/bidsmith"
  export PATH="$__bs_dir:$PATH"
fi
unset __bs_dir __bs_arch __bs_tgt __bs_cached
