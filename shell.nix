{ pkgs ? import <nixpkgs> { } }:
let
  runtimeLibs = with pkgs; [
    wayland libxkbcommon vulkan-loader fontconfig freetype openssl zstd
    libx11 libxcb libxcursor libxrandr libxi
  ];
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer pkg-config cmake clang mold git gh ];
  buildInputs = runtimeLibs;
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
}
