# synclaude

Synchronize `~/.claude/` directories across NixOS machines via git.

## Features

- **Git-based Synchronization:** Backs up and syncs your Claude configuration using a remote Git repository.
- **Machine-specific Branches:** Each machine syncs to its own branch (based on `/etc/machine-id` or a custom name).
- **Background Daemon:** Runs a background daemon that watches for file changes and periodically pulls from the remote.
- **Native Git Integration:** Uses `gix` (gitoxide) for fast, pure-Rust Git operations.

## Installation

Ensure you have Rust and Cargo installed.

```bash
cargo install --path .
```

### Nix / Nix Flakes

If you use Nix with flakes enabled, you can run or install it directly:

```bash
# Run without installing
nix run github:connerohnesorge/synclaude

# Or run from the local directory
nix run .

# Install to your profile
nix profile install .
```

You can also use it in your system configuration or home-manager by adding it as an input to your flake.

```nix
{
  inputs = {
    synclaude.url = "github:connerohnesorge/synclaude";
  };

  outputs = { self, nixpkgs, synclaude, ... }: {
    nixosConfigurations.myHost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        # ... your configuration
        { environment.systemPackages = [ synclaude.packages.x86_64-linux.default ]; }
      ];
    };
  };
}
```

## Usage

### Initialization

First, initialize `synclaude` with a remote Git repository:

```bash
synclaude init <REPO_URL>
```
*Optional: You can override the machine name with `--machine-name <NAME>`.*

### Manual Synchronization

You can manually push or pull changes:

```bash
# Push local changes to the remote
synclaude push

# Pull and merge remote changes
synclaude pull
```

### Background Daemon

To run the file watcher and periodic puller in the background:

```bash
synclaude daemon
```

### Status

To check the current configuration and sync status:

```bash
synclaude status
```

## Development

- Built with Rust 2024 edition.
- Uses `clap` for CLI parsing, `notify` for file watching, and `gix` for Git operations.

## License

*(Add license information here)*
