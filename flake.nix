{
  description = "ht";

  inputs = {
    nixpkgs.url = github:nixos/nixpkgs/nixpkgs-unstable;
    flake-utils.url = github:numtide/flake-utils;
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rustup
              bashInteractive
            ] ++ (lib.optionals stdenv.isDarwin [
              libiconv
              darwin.apple_sdk.frameworks.Foundation
            ]);

            RUSTUP_TOOLCHAIN = "1.74";
          };
        }
      );
}
