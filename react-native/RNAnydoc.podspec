require "json"

package = JSON.parse(File.read(File.join(__dir__, "package.json")))

Pod::Spec.new do |s|
  s.name         = "RNAnydoc"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = package["homepage"]
  s.license      = package["license"]
  s.authors      = { "Tula" => "https://github.com/tulaafrica" }
  s.platforms    = { :ios => "15.1" }
  s.source       = { :git => "https://github.com/tulaafrica/anydoc.git", :tag => "rn-v#{package["version"]}" }

  # The Nitro glue. The Rust core arrives prebuilt as an XCFramework
  # (device + simulator slices) — scripts/build-rust-ios.sh produces it,
  # and consumers get it from the GitHub Release via the prepare flow
  # described in the README (it is far too large for the npm tarball).
  s.source_files = ["cpp/**/*.{h,hpp,cpp}"]
  # cpp-adapter.cpp is the Android JNI_OnLoad; iOS registration goes through
  # nitrogen's generated Swift autolinking instead.
  s.exclude_files = ["cpp/cpp-adapter.cpp"]
  s.vendored_frameworks = "ios/AnydocCore.xcframework"

  # Everything nitrogen generated (specs, Swift bridges, autolinking).
  load File.join(__dir__, "nitrogen/generated/ios/RNAnydoc+autolinking.rb")
  add_nitrogen_files(s)

  # React/JSI dependencies, wired the way the host RN version expects.
  if respond_to?(:install_modules_dependencies, true)
    install_modules_dependencies(s)
  else
    s.dependency "React-Core"
  end
end
