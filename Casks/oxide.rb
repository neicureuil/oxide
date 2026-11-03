cask "oxide" do
  version "0.8.0"
  sha256 "9d29297d4dba694d9c29804d454bb969a8dbe69a6938866eec89c572ed5686a2"

  url "https://github.com/neicureuil/oxide/releases/download/v#{version}/Oxide-#{version}.zip"
  name "Oxide"
  desc "Unofficial native client for Microsoft Teams"
  homepage "https://github.com/neicureuil/oxide"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on arch: :arm64
  depends_on macos: :sonoma

  app "Oxide.app"

  zap trash: [
    "~/Library/Caches/fr.neicureuil.oxide",
    "~/Library/Caches/Oxide",
    "~/Library/HTTPStorages/fr.neicureuil.oxide",
    "~/Library/Preferences/fr.neicureuil.oxide.plist",
    "~/Library/WebKit/fr.neicureuil.oxide",
  ]

  caveats <<~EOS
    Oxide is not notarized: macOS refuses to open it after each install or upgrade.
    Allow it in System Settings › Privacy & Security, or run:
      xattr -dr com.apple.quarantine #{appdir}/Oxide.app
  EOS
end
