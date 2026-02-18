{
  description = "daft - Git extensions toolkit for worktree management";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
    }:
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ]
      (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          craneLib = crane.mkLib pkgs;

          # Only include Rust-relevant source files for better caching
          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;

            buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            ];

            nativeBuildInputs = [
              pkgs.pkg-config
            ];
          };

          # Build dependencies separately for caching
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          daft = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;

              # Only run lib/unit tests (integration tests need a real git env)
              cargoTestExtraArgs = "--lib";

              postInstall = ''
                # Create symlinks for the multicall binary
                cd $out/bin
                for cmd in \
                  git-worktree-clone \
                  git-worktree-init \
                  git-worktree-checkout \
                  git-worktree-checkout-branch \
                  git-worktree-prune \
                  git-worktree-carry \
                  git-worktree-fetch \
                  git-worktree-flow-adopt \
                  git-worktree-flow-eject \
                  git-daft; do
                  ln -s daft "$cmd"
                done

                # Install pre-generated man pages
                mkdir -p $out/share/man/man1
                cp ${./man}/*.1 $out/share/man/man1/
              '';

              meta = with pkgs.lib; {
                description = "Git extensions toolkit for powerful worktree management";
                homepage = "https://github.com/avihut/daft";
                license = licenses.mit;
                maintainers = [ ];
                platforms = platforms.unix;
                mainProgram = "daft";
              };
            }
          );
        in
        {
          packages = {
            default = daft;
            daft = daft;
          };

          apps.default = flake-utils.lib.mkApp {
            drv = daft;
          };

          checks = {
            inherit daft;
          };

          devShells.default = craneLib.devShell {
            checks = self.checks.${system};
            packages = [
              pkgs.cargo-watch
            ];
          };
        }
      );
}
