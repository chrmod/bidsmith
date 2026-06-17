class Bidsmith < Formula
  desc "Declarative, AI-friendly tooling for Google Ads campaigns"
  homepage "https://github.com/chrmod/bidsmith"
  version "0.15.0"
  license "MPL-2.0"

  on_macos do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-apple-darwin.tar.gz"
      sha256 "ef66fc9138d652019256c1305071eb9f316b34bb917c68e1b4bf9749649cee8a"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-apple-darwin.tar.gz"
      sha256 "5c172a49add62f16df108ade7ab1a06ee6a443059f04618aff00176f53fc2538"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1c03d08b66aebbbb77515e428637affb5e9abb7820d69812172f845bcd47dc5a"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "f33b24457aa265ff7ecb0879b87c82a115867a916fbf72a912fd135c06df159a"
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
