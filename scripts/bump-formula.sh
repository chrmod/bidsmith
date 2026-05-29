#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <version>   (e.g. 0.1.0 — no leading v)" >&2
  exit 2
fi

VERSION="$1"
TAG="v${VERSION}"
REPO="chrmod/bidsmith"
FORMULA="$(cd "$(dirname "$0")/.." && pwd)/homebrew/bidsmith.rb"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fetch_sha() {
  local target="$1"
  local url="https://github.com/${REPO}/releases/download/${TAG}/bidsmith-${target}.tar.gz"
  echo "fetching ${url}" >&2
  curl -fsSL "$url" -o "${tmpdir}/${target}.tar.gz"
  shasum -a 256 "${tmpdir}/${target}.tar.gz" | awk '{print $1}'
}

sha_macos_arm="$(fetch_sha aarch64-apple-darwin)"
sha_macos_intel="$(fetch_sha x86_64-apple-darwin)"
sha_linux_arm="$(fetch_sha aarch64-unknown-linux-gnu)"
sha_linux_intel="$(fetch_sha x86_64-unknown-linux-gnu)"

cat > "$FORMULA" <<EOF
class Bidsmith < Formula
  desc "Declarative, AI-friendly tooling for Google Ads campaigns"
  homepage "https://github.com/chrmod/bidsmith"
  version "${VERSION}"
  license "MPL-2.0"

  on_macos do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-apple-darwin.tar.gz"
      sha256 "${sha_macos_arm}"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-apple-darwin.tar.gz"
      sha256 "${sha_macos_intel}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${sha_linux_arm}"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${sha_linux_intel}"
    end
  end

  def install
    bin.install "bidsmith"
    system "codesign", "--force", "--sign", "-", bin/"bidsmith" if OS.mac?
  end

  test do
    assert_match "validate", shell_output("#{bin}/bidsmith --help")
  end
end
EOF

echo
echo "updated ${FORMULA} to ${VERSION}"
echo "next: copy it into chrmod/homebrew-tap (Formula/bidsmith.rb) and push"
