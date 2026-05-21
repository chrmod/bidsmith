class Bidsmith < Formula
  desc "Declarative, AI-friendly tooling for Google Ads campaigns"
  homepage "https://github.com/chrmod/bidsmith"
  version "0.7.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-apple-darwin.tar.gz"
      sha256 "1360282040a3f2930c635e507c6ecde40b0c84af3354bdae40448bd8221f1ed0"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-apple-darwin.tar.gz"
      sha256 "2f6fe196dbf4f36b4842c5ac9f157f38a5a7488ad7170b93e0f4d712cc807c73"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f135954613d0abf9ddfc238e7faf931a85a1be2558979056d6a6cdb147048f07"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "32d0d0c3b27c113353035b3e3c87805c629dd58600851e53e2e41571a19b1e63"
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
