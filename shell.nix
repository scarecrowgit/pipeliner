{ pkgs ? import <nixpkgs> { } }:
let
  libs = with pkgs; [
    wayland
    libxkbcommon
    vulkan-loader
    fontconfig
    freetype
    openssl
    zstd
    libx11
    libxcb
    libgit2
  ];
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer pkg-config cmake clang mold ];
  buildInputs = libs;
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
}
