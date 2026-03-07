# NixOS VM integration tests for synclaude
#
# Test topology:
#   - gitserver: bare git repo served over SSH
#   - machineA: first synclaude client
#   - machineB: second synclaude client
#
# Run with: nix build .#checks.x86_64-linux.integration
{
  pkgs,
  self,
}: let
  # SSH key pair generated at build time for test-only use
  sshKeygen = pkgs.runCommand "test-ssh-keys" {} ''
    mkdir -p $out
    ${pkgs.openssh}/bin/ssh-keygen -t ed25519 -f $out/id_ed25519 -N "" -C "test@synclaude"
  '';

  testUser = "testuser";

  commonClientConfig = {machineId}: {
    pkgs,
    lib,
    ...
  }: {
    users.users.${testUser} = {
      isNormalUser = true;
      home = "/home/${testUser}";
      openssh.authorizedKeys.keyFiles = ["${sshKeygen}/id_ed25519.pub"];
    };

    environment.systemPackages = [
      self.packages.${pkgs.system}.default
      pkgs.git
    ];

    # Give each VM a unique machine-id
    environment.etc."machine-id".text = machineId;

    # Pre-configure SSH to trust the git server without prompts
    programs.ssh.extraConfig = ''
      Host gitserver
        StrictHostKeyChecking no
        UserKnownHostsFile /dev/null
        IdentityFile ${sshKeygen}/id_ed25519
        User git
        LogLevel ERROR
    '';
  };
in
  pkgs.testers.runNixOSTest {
    name = "synclaude-integration";

    nodes = {
      gitserver = {
        pkgs,
        lib,
        ...
      }: {
        networking.firewall.allowedTCPPorts = [22];

        services.openssh = {
          enable = true;
          settings = {
            PasswordAuthentication = false;
          };
        };

        users.users.git = {
          isNormalUser = true;
          home = "/home/git";
          shell = "${pkgs.git}/bin/git-shell";
          openssh.authorizedKeys.keyFiles = ["${sshKeygen}/id_ed25519.pub"];
        };

        environment.systemPackages = [pkgs.git];
      };

      machineA = commonClientConfig {machineId = "aaaa1111aaaa1111aaaa1111aaaa1111";};
      machineB = commonClientConfig {machineId = "bbbb2222bbbb2222bbbb2222bbbb2222";};
    };

    testScript = ''
      import time

      start_all()

      # ── 1. Set up the bare git repo on the server ──────────────────────
      with subtest("Set up bare git repo on server"):
          gitserver.wait_for_unit("sshd.service")
          gitserver.succeed(
              "su - git -s /bin/sh -c '"
              "git init --bare /home/git/synclaude.git && "
              "cd /home/git/synclaude.git && "
              "git symbolic-ref HEAD refs/heads/main"
              "'"
          )

      # ── 2. Configure git identity on both clients ──────────────────────
      for machine in [machineA, machineB]:
          machine.wait_for_unit("multi-user.target")
          machine.succeed(
              "su - ${testUser} -c '"
              "git config --global user.email test@synclaude && "
              "git config --global user.name synclaude-test"
              "'"
          )

      # ── 3. Test: synclaude init on machineA ────────────────────────────
      with subtest("synclaude init on machineA"):
          machineA.succeed(
              "su - ${testUser} -c '"
              "synclaude init ssh://git@gitserver/home/git/synclaude.git"
              "'"
          )
          # Verify config was created
          machineA.succeed(
              "su - ${testUser} -c 'test -f /home/${testUser}/.config/synclaude/config.toml'"
          )
          # Verify local repo was created
          machineA.succeed(
              "su - ${testUser} -c 'test -d /home/${testUser}/.local/share/synclaude/repo/.git'"
          )

      # ── 4. Test: synclaude status on machineA ──────────────────────────
      with subtest("synclaude status on machineA"):
          output = machineA.succeed(
              "su - ${testUser} -c 'synclaude status'"
          )
          assert "aaaa1111" in output, f"Expected machine ID in status output, got: {output}"
          assert "machine/aaaa1111" in output, f"Expected branch name in status, got: {output}"

      # ── 5. Test: push from machineA ────────────────────────────────────
      with subtest("Create files and push from machineA"):
          # Create test content in ~/.claude/projects/
          machineA.succeed(
              "su - ${testUser} -c '"
              "mkdir -p /home/${testUser}/.claude/projects/myproject/memory && "
              "echo hello-from-A > /home/${testUser}/.claude/projects/myproject/memory/MEMORY.md && "
              "mkdir -p /home/${testUser}/.claude/todos && "
              "echo buy-milk > /home/${testUser}/.claude/todos/todo1.md && "
              "mkdir -p /home/${testUser}/.claude/plans && "
              "echo grand-plan > /home/${testUser}/.claude/plans/plan1.md"
              "'"
          )
          # Push
          machineA.succeed(
              "su - ${testUser} -c 'synclaude push'"
          )
          # Verify the remote received the push
          gitserver.succeed(
              "su - git -s /bin/sh -c '"
              "cd /home/git/synclaude.git && "
              "git branch | grep machine/aaaa1111"
              "'"
          )

      # ── 6. Test: init + pull on machineB ───────────────────────────────
      with subtest("synclaude init and pull on machineB"):
          machineB.succeed(
              "su - ${testUser} -c '"
              "synclaude init ssh://git@gitserver/home/git/synclaude.git"
              "'"
          )
          # The repo should have been cloned with machineA's data
          machineB.succeed(
              "su - ${testUser} -c 'synclaude pull'"
          )
          # Verify the files arrived in ~/.claude/
          output = machineB.succeed(
              "su - ${testUser} -c 'cat /home/${testUser}/.claude/projects/myproject/memory/MEMORY.md'"
          )
          assert "hello-from-A" in output, f"Expected synced content, got: {output}"

          output = machineB.succeed(
              "su - ${testUser} -c 'cat /home/${testUser}/.claude/todos/todo1.md'"
          )
          assert "buy-milk" in output, f"Expected todo content, got: {output}"

          output = machineB.succeed(
              "su - ${testUser} -c 'cat /home/${testUser}/.claude/plans/plan1.md'"
          )
          assert "grand-plan" in output, f"Expected plan content, got: {output}"

      # ── 7. Test: push from machineB, pull on machineA ──────────────────
      with subtest("Bidirectional sync: B pushes, A pulls"):
          machineB.succeed(
              "su - ${testUser} -c '"
              "echo hello-from-B > /home/${testUser}/.claude/projects/myproject/memory/FROM_B.md"
              "'"
          )
          machineB.succeed(
              "su - ${testUser} -c 'synclaude push'"
          )
          # Verify B's branch exists on remote
          gitserver.succeed(
              "su - git -s /bin/sh -c '"
              "cd /home/git/synclaude.git && "
              "git branch | grep machine/bbbb2222"
              "'"
          )

      # ── 8. Test: concurrent edits to different files ───────────────────
      with subtest("Concurrent edits to different files"):
          # A edits file1, B edits file2 — no conflict
          machineA.succeed(
              "su - ${testUser} -c '"
              "echo updated-by-A > /home/${testUser}/.claude/projects/myproject/memory/MEMORY.md"
              "'"
          )
          machineA.succeed("su - ${testUser} -c 'synclaude push'")

          machineB.succeed(
              "su - ${testUser} -c '"
              "echo new-todo-from-B > /home/${testUser}/.claude/todos/todo2.md"
              "'"
          )
          machineB.succeed("su - ${testUser} -c 'synclaude push'")

          # Both branches should exist and have their respective changes
          gitserver.succeed(
              "su - git -s /bin/sh -c '"
              "cd /home/git/synclaude.git && "
              "git log --all --oneline | head -10"
              "'"
          )

      # ── 9. Test: daemon auto-push via file watcher ─────────────────────
      with subtest("Daemon auto-push on file change"):
          # Start daemon in background on machineA using shell backgrounding
          machineA.succeed(
              "su - ${testUser} -c 'RUST_LOG=debug synclaude daemon &disown' >/dev/null 2>&1"
          )
          # Give the daemon time to start the file watcher
          time.sleep(3)

          # Create a new file — the daemon should auto-commit+push
          machineA.succeed(
              "su - ${testUser} -c '"
              "echo daemon-test > /home/${testUser}/.claude/projects/daemon-test.md"
              "'"
          )

          # Wait for the debounce window (5s) + push time
          time.sleep(15)

          # Check that the remote got a new commit on machineA's branch
          result = gitserver.succeed(
              "su - git -s /bin/sh -c '"
              "cd /home/git/synclaude.git && "
              "git log machine/aaaa1111aaaa1111aaaa1111aaaa1111 --oneline"
              "'"
          )
          assert "auto-sync" in result, f"Expected auto-sync commit, got: {result}"

      # ── 10. Test: status shows all sync dirs ────────────────────────────
      with subtest("Status shows correct sync dir state"):
          output = machineA.succeed(
              "su - ${testUser} -c 'synclaude status'"
          )
          assert "projects: exists" in output, f"Expected projects exists, got: {output}"
          assert "todos: exists" in output, f"Expected todos exists, got: {output}"
          assert "plans: exists" in output, f"Expected plans exists, got: {output}"
    '';
  }
