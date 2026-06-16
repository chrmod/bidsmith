class Bidsmith < Formula
  desc "Declarative, AI-friendly tooling for Google Ads campaigns"
  homepage "https://github.com/chrmod/bidsmith"
  version "0.14.1"
  license "MPL-2.0"

  on_macos do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-apple-darwin.tar.gz"
      sha256 "6e5bc7792f4374dc2f3cd46d2237eee95082fa35a2103dde5cb5878e05d47a40"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-apple-darwin.tar.gz"
      sha256 "d79feac582621ec965a1eeb98907321d6997944e36eaaf5b6f98daf52f66225d"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "26e0c3cb6a278f151501be8cc780bee38c045696b041f152b672d77270ab291c"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8cd58ede862a4cc4d5530ddf898a9dbbed8dae07fd3c524a49078a4dfe448347"
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
