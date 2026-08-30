{
  description = "mandible: a universal, interactive TUI reference for CLI tools";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      eachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = eachSystem (pkgs: rec {
        mandible = pkgs.rustPlatform.buildRustPackage {
          pname = "mandible";
          # Single source of truth: the workspace version (spec: crates are
          # bumped together with workspace.package.version).
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "--package"
            "mandible"
          ];
          nativeBuildInputs = [ pkgs.installShellFiles ];
          # The workspace's own suite runs in the repository's CI. The nix
          # sandbox cannot provide what parts of it require (a pty, and the
          # unprivileged user namespaces exec::containment builds on), so
          # this derivation packages the binary rather than re-running
          # verification that already gates every merge.
          doCheck = false;
          postInstall = pkgs.lib.optionalString (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
            installShellCompletion --cmd mandible \
              --bash <($out/bin/mandible --completions bash) \
              --zsh <($out/bin/mandible --completions zsh) \
              --fish <($out/bin/mandible --completions fish)
          '';
          meta = {
            description = "Universal, interactive TUI reference for CLI tools";
            homepage = "https://github.com/AS-FOSS/mandible";
            changelog = "https://github.com/AS-FOSS/mandible/blob/main/CHANGELOG.md";
            license = with pkgs.lib.licenses; [
              asl20
              mit
            ];
            mainProgram = "mandible";
          };
        };
        default = mandible;
      });
    };
}
