class Bidsmith < Formula
  desc "Declarative, AI-friendly tooling for Google Ads campaigns"
  homepage "https://github.com/chrmod/bidsmith"
  version "0.21.1"
  license "MPL-2.0"

  on_macos do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-apple-darwin.tar.gz"
      sha256 "b17f46b19e2003dfdf98c55001d4d9f8775dcfe02cd82ae670b7a6f47e7ac04d"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-apple-darwin.tar.gz"
      sha256 "4aaf973388ac0c60fc86900d5e665d0451d35e61510541565091a9f1ef5c9509"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "08d4f6fba400ab6d8c15213259aca6dede1d4429d8a8e49844f88560722a21e0"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "2c4d405d6dd2645ef73d69129c7f5af43edb7ddf36f25dad2290e7019547973f"
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
