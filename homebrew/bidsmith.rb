class Bidsmith < Formula
  desc "Declarative, AI-friendly tooling for Google Ads campaigns"
  homepage "https://github.com/chrmod/bidsmith"
  version "0.16.0"
  license "MPL-2.0"

  on_macos do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-apple-darwin.tar.gz"
      sha256 "2cbce6a217b413d7fa0bac5e769de31d2931133a3b55cb2f2a4fdeadc4e97ae0"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-apple-darwin.tar.gz"
      sha256 "d0d47a333da36ff21686c985e8f08c212753227945e15e55ae13718e70f993ae"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "2d4c1a39e0d9a9d02c242f5ff9ad87c141d6a7d82ef36bfe0049006e554bebc4"
    end
    on_intel do
      url "https://github.com/chrmod/bidsmith/releases/download/v#{version}/bidsmith-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "afd102b31f1cb501fd44306ee25bf428132c9e243c0a23174d9bebea20c2b103"
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
