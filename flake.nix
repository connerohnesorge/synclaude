{
  description = "synclaude - Synchronize ~/.claude/ across NixOS machines via git";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    crane,
    treefmt-nix,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [rust-overlay.overlays.default];
      };

      craneLib = (crane.mkLib pkgs).overrideToolchain (p: p.rust-bin.stable.latest.default);

      commonArgs = {
        src = craneLib.cleanCargoSource ./.;
        strictDeps = true;
        buildInputs = [];
        nativeBuildInputs = [];
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      synclaude = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
        });

      rooted = exec:
        builtins.concatStringsSep "\n"
        [
          ''REPO_ROOT="$(git rev-parse --show-toplevel)"''
          exec
        ];

      scripts = {
        dx = {
          exec = rooted ''$EDITOR "$REPO_ROOT"/flake.nix'';
          description = "Edit flake.nix";
        };
        rx = {
          exec = rooted ''$EDITOR "$REPO_ROOT"/Cargo.toml'';
          description = "Edit Cargo.toml";
        };
      };

      scriptPackages =
        pkgs.lib.mapAttrs
        (
          name: script:
            pkgs.writeShellApplication {
              inherit name;
              text = script.exec;
              runtimeInputs = script.deps or [];
            }
        )
        scripts;
    in {
      checks = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
        integration = import ./tests/nixos {
          inherit pkgs self;
        };
      };

      packages = {
        default = synclaude;
        synclaude = synclaude;
      };

      devShells.default = craneLib.devShell {
        name = "dev";
        inputsFrom = [synclaude];
        packages = with pkgs;
          [
            alejandra
            nixd
            statix
            deadnix
            just
            rust-bin.stable.latest.rust-analyzer
          ]
          ++ builtins.attrValues scriptPackages;
        shellHook = ''
          echo "Welcome to the synclaude devshell!"
        '';
      };

      formatter = let
        treefmtModule = {
          projectRootFile = "flake.nix";
          programs = {
            alejandra.enable = true;
            rustfmt.enable = true;
          };
        };
      in
        treefmt-nix.lib.mkWrapper pkgs treefmtModule;
    })
    // {
      nixosModules.default = {
        config,
        lib,
        pkgs,
        ...
      }: let
        cfg = config.services.synclaude;
      in {
        options.services.synclaude = {
          enable = lib.mkEnableOption "synclaude directory sync daemon";

          package = lib.mkOption {
            type = lib.types.package;
            default = self.packages.${pkgs.system}.default;
            description = "The synclaude package to use.";
          };

          remoteUrl = lib.mkOption {
            type = lib.types.str;
            description = "Git remote URL for the sync repository.";
          };

          user = lib.mkOption {
            type = lib.types.str;
            description = "User account to run synclaude as.";
          };

          pullIntervalSecs = lib.mkOption {
            type = lib.types.int;
            default = 300;
            description = "Interval in seconds for periodic pull.";
          };

          syncDirs = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = ["projects" "todos" "plans"];
            description = "Subdirectories of ~/.claude/ to sync.";
          };
        };

        config = lib.mkIf cfg.enable {
          systemd.services.synclaude = {
            description = "synclaude - Claude directory sync daemon";
            after = ["network-online.target"];
            wants = ["network-online.target"];
            wantedBy = ["multi-user.target"];

            serviceConfig = {
              Type = "simple";
              User = cfg.user;
              ExecStartPre = let
                initScript = pkgs.writeShellScript "synclaude-init" ''
                  CONFIG_DIR="$HOME/.config/synclaude"
                  if [ ! -f "$CONFIG_DIR/config.toml" ]; then
                    ${cfg.package}/bin/synclaude init "${cfg.remoteUrl}"
                  fi
                '';
              in "${initScript}";
              ExecStart = "${cfg.package}/bin/synclaude daemon";
              Restart = "on-failure";
              RestartSec = 30;
            };

            environment = {
              RUST_LOG = "info";
            };
          };
        };
      };
    };
}
