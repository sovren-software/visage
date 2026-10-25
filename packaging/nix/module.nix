# Visage — NixOS module
#
# Usage in your NixOS configuration:
#
#   imports = [ visage.nixosModules.default ];
#
#   services.visage = {
#     enable = true;
#     # modelDir = "/var/lib/visage/models";  # default
#     # logLevel = "visaged=info";             # default
#   };
#
# This module:
#   - Installs visage, visaged, and the PAM module
#   - Creates and manages /var/lib/visage with correct permissions
#   - Registers the D-Bus system bus policy
#   - Enables the visaged systemd service (hardened)
#   - Enables the suspend/resume restart service
#   - Configures PAM for face authentication (before password, with fallback)

{ config, lib, pkgs, ... }:

let
  cfg = config.services.visage;

  # Module arguments shared by every PAM rule Visage installs. `settings` is
  # preferred over `args` because NixOS renders it into the same module-argument
  # list while leaving each entry individually overridable by the consumer.
  pamSettings = lib.optionalAttrs (cfg.pam.timeoutSeconds != null) {
    timeout = cfg.pam.timeoutSeconds;
  };
  visagePkg = pkgs.callPackage ./default.nix { };
in
{
  options.services.visage = {
    enable = lib.mkEnableOption "Visage face authentication daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = visagePkg;
      defaultText = lib.literalExpression "pkgs.visage";
      description = "The Visage package to use.";
    };

    modelDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/visage/models";
      description = "Directory containing ONNX face detection and recognition models.";
    };

    dbPath = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/visage/faces.db";
      description = "Path to the SQLite face embedding database.";
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "visaged=info";
      description = "RUST_LOG filter string for the daemon.";
    };

    camera = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/dev/video2";
      description = ''
        Camera device path. When null, the daemon auto-detects the first
        available V4L2 capture device.
      '';
    };

    similarityThreshold = lib.mkOption {
      type = lib.types.nullOr lib.types.float;
      default = null;
      example = 0.45;
      description = ''
        Cosine similarity threshold for face matching. Higher values are
        stricter. When null, the daemon uses its compiled default (0.40).
      '';
    };

    framesPerVerify = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      example = 2;
      description = ''
        Number of frames captured and scored per verification. When null, the
        daemon uses its compiled default (3).

        This is the main latency knob: each frame costs one SCRFD detection
        plus one ArcFace embedding, which dominates verification time on
        CPU-only hardware. Lowering it speeds up every `sudo` and every screen
        unlock.

        Two interactions to weigh before lowering it:

        - `pam-visage` applies a hard 3-second D-Bus timeout. If verification
          exceeds that, PAM falls through to the password prompt, and the
          result is indistinguishable from a failed match. Measure your
          verification latency before assuming you have headroom.
        - Passive liveness needs at least two frames in which a face was
          detected, and fails closed below that. At `framesPerVerify = 2`, a
          single frame that misses detection leaves one landmark frame and the
          attempt is rejected. Leave liveness disabled, or keep three frames,
          if that trade is not acceptable.
      '';
    };

    framesPerEnroll = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      example = 5;
      description = ''
        Number of frames captured per enrollment. The highest-confidence face
        across these frames is the one embedded. When null, the daemon uses
        its compiled default (5).
      '';
    };

    warmupFrames = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.unsigned;
      default = null;
      example = 4;
      description = ''
        Frames discarded at engine startup so the camera's auto-exposure and
        auto-gain can stabilise. When null, the daemon uses its compiled
        default (4).

        This cost is paid once when the daemon opens the camera, not per
        verification, so raising it does not slow down authentication.
      '';
    };

    framingMs = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.unsigned;
      default = null;
      example = 1500;
      description = ''
        How long, in milliseconds, to stream frames to an enrolling client
        before capturing anything, so the user can see themselves and get
        centred. When null, the daemon uses its compiled default (1500).
        Set to 0 to disable the framing phase entirely.

        Only clients that subscribe to `PreviewFrame` pay this cost — a
        scripted or headless enrollment skips the phase, so it adds no
        latency to automated setup.

        ⚠️ Not purely cosmetic. Sensor auto-gain adapts only while the camera
        is streaming, so holding the stream open for framing acts as an
        extended warmup and leaves the sensor in a different state than a bare
        capture would. That is likely an improvement, but it is a real change
        to capture conditions rather than a no-op.
      '';
    };

    verifyTimeoutSeconds = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      example = 10;
      description = ''
        Deadline for a single verification inside the daemon. When null, the
        daemon uses its compiled default (10).

        Note this is the *daemon-side* bound. `pam-visage` independently
        applies a 3-second D-Bus method timeout, so raising this above 3 has
        no effect on the PAM path — PAM gives up first and falls back to the
        password.
      '';
    };

    inference.cpuThreads = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      example = 4;
      description = ''
        ONNX Runtime threads per model on the CPU. When null, the daemon uses
        its compiled default (2). Around the number of physical cores is a
        good start; measure with the verify timing line.
      '';
    };

    emitter.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to drive the camera's IR emitter via its UVC extension unit,
        for cameras with a matching quirk in `contrib/hw/`.

        Disabling this does not necessarily mean the sensor goes dark: some
        modules enable IR illumination in firmware and strobe it regardless of
        any quirk. Check the per-frame brightness sequence
        (`visage test -n 20`) rather than assuming — a dark-frame *count*
        cannot distinguish a strobing emitter from an exposure ramp.
      '';
    };

    liveness.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to enable passive liveness detection (landmark stability).
        When enabled, static photographs are rejected by checking that eye
        landmarks shift between captured frames.
      '';
    };

    liveness.minDisplacement = lib.mkOption {
      type = lib.types.nullOr lib.types.float;
      default = null;
      example = 0.8;
      description = ''
        Minimum mean eye landmark displacement (in pixels) for the liveness
        check. Lower values are more permissive. When null, the daemon uses
        its compiled default (0.8).
      '';
    };

    pam.timeoutSeconds = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      example = 6;
      description = ''
        D-Bus method timeout applied by `pam_visage.so`, in seconds. When
        null, the module's compiled default (3) is used.

        This is an upper bound on how long authentication may take, not only a
        hang guard: if verification exceeds it, PAM falls through to the
        password prompt and the outcome is **indistinguishable from a failed
        match** — both return `PAM_IGNORE`, and nothing in the PAM log says
        "timed out".

        On CPU-only hardware a verify can take seconds. Measure yours before
        assuming the default fits; if median latency is anywhere near 3s, the
        result is intermittent password prompts that look like recognition
        failures. Raising this trades a longer worst-case hang, when the daemon
        is genuinely stuck, for not being cut off mid-verification.
      '';
    };

    pam.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to enable Visage PAM integration. When enabled, face
        authentication is tried before password for `sudo` and `login`, with
        the password always available as fallback.

        Screen lock is covered only *indirectly*, and whether it works depends
        on your locker:

        - Lockers that derive their PAM stack from `login` — DMS/quickshell
          generates a config with an identical module set — inherit face auth
          with no extra configuration.
        - Lockers that declare their own PAM service get nothing from this
          option and must be wired explicitly:

          ```nix
          security.pam.services.<service>.rules.auth.visage = {
            order = 900;
            control = "[success=done default=ignore]";
            modulePath = "${cfg.package}/lib/security/pam_visage.so";
          };
          ```

        Do not audit this by grepping `/etc/pam.d/` alone — a locker's config
        may live in the user's state directory instead. The running service
        name is in the journal:
        `journalctl | grep 'Starting pam session' | tail -1`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # Make CLI available system-wide
    environment.systemPackages = [ cfg.package ];

    # D-Bus system bus policy — allows daemon to own the bus name,
    # restricts mutation methods to root, allows verify/status for all users
    services.dbus.packages = [ cfg.package ];

    # State directory
    systemd.tmpfiles.rules = [
      "d /var/lib/visage 0700 root root -"
      "d ${cfg.modelDir} 0700 root root -"
    ];

    # Main daemon service
    systemd.services.visaged = {
      description = "Visage biometric authentication daemon";
      after = [ "dbus.service" ];
      requires = [ "dbus.service" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        VISAGE_MODEL_DIR = toString cfg.modelDir;
        VISAGE_DB_PATH = toString cfg.dbPath;
        RUST_LOG = cfg.logLevel;
      } // lib.optionalAttrs (cfg.camera != null) {
        VISAGE_CAMERA_DEVICE = cfg.camera;
      } // lib.optionalAttrs (cfg.similarityThreshold != null) {
        VISAGE_SIMILARITY_THRESHOLD = toString cfg.similarityThreshold;
      } // lib.optionalAttrs (!cfg.liveness.enable) {
        VISAGE_LIVENESS_ENABLED = "0";
      } // lib.optionalAttrs (cfg.liveness.minDisplacement != null) {
        VISAGE_LIVENESS_MIN_DISPLACEMENT = toString cfg.liveness.minDisplacement;
      } // lib.optionalAttrs (cfg.framesPerVerify != null) {
        VISAGE_FRAMES_PER_VERIFY = toString cfg.framesPerVerify;
      } // lib.optionalAttrs (cfg.framesPerEnroll != null) {
        VISAGE_FRAMES_PER_ENROLL = toString cfg.framesPerEnroll;
      } // lib.optionalAttrs (cfg.warmupFrames != null) {
        VISAGE_WARMUP_FRAMES = toString cfg.warmupFrames;
      } // lib.optionalAttrs (cfg.framingMs != null) {
        VISAGE_FRAMING_MS = toString cfg.framingMs;
      } // lib.optionalAttrs (cfg.verifyTimeoutSeconds != null) {
        VISAGE_VERIFY_TIMEOUT_SECS = toString cfg.verifyTimeoutSeconds;
      } // lib.optionalAttrs (cfg.inference.cpuThreads != null) {
        VISAGE_CPU_THREADS = toString cfg.inference.cpuThreads;
      } // lib.optionalAttrs (!cfg.emitter.enable) {
        VISAGE_EMITTER_ENABLED = "0";
      };

      serviceConfig = {
        Type = "simple";
        ExecStart = "${cfg.package}/bin/visaged";
        Restart = "on-failure";
        RestartSec = 5;

        # Bounds a stuck v4l2 capture on `systemctl stop|restart`. visaged
        # handles SIGINT/SIGTERM at the runtime layer (issue #26); this covers
        # the case where a capture is mid-flight and not promptly interruptible,
        # e.g. after hibernate resume with a stale camera fd.
        #
        # This mirrors packaging/systemd/visaged.service, which has carried it
        # since #26. It was missing here, so NixOS hosts fell back to systemd's
        # 90s default — precisely the hang the fix removed on Debian. Found by
        # tests/systemd_hardening_contract.rs.
        TimeoutStopSec = 10;

        # Hardening (mirrors packaging/systemd/visaged.service)
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        DeviceAllow = [ "char-video4linux rw" ];
        ReadWritePaths = [ "/var/lib/visage" ];
        CapabilityBoundingSet = "";
        SystemCallArchitectures = "native";
        # Enforces the "no network access" claim threat-model.md relies on to
        # justify MemoryDenyWriteExecute=false. See issue #78 and the comment
        # in packaging/systemd/visaged.service. visaged makes no network calls;
        # `ureq` lives only in visage-cli, for `visage setup`.
        PrivateNetwork = true;
        MemoryDenyWriteExecute = false;
      };
    };

    # Restart daemon after resume from suspend/hibernate
    systemd.services.visage-resume = {
      description = "Restart Visage daemon after resume from suspend";
      after = [
        "suspend.target"
        "hibernate.target"
        "hybrid-sleep.target"
        "suspend-then-hibernate.target"
      ];
      wantedBy = [
        "suspend.target"
        "hibernate.target"
        "hybrid-sleep.target"
        "suspend-then-hibernate.target"
      ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${pkgs.systemd}/bin/systemctl restart visaged.service";
      };
    };

    # PAM integration — face auth before password, password fallback
    security.pam.services = lib.mkIf cfg.pam.enable {
      sudo.rules.auth.visage = {
        order = 900;
        control = "[success=done default=ignore]";
        modulePath = "${cfg.package}/lib/security/pam_visage.so";
        settings = pamSettings;
      };
      login.rules.auth.visage = {
        order = 900;
        control = "[success=done default=ignore]";
        modulePath = "${cfg.package}/lib/security/pam_visage.so";
        settings = pamSettings;
      };
      # Screen lockers (swaylock, hyprlock, etc.) use their own PAM service.
      # Users can add more via:
      #   security.pam.services.<name>.rules.auth.visage = { ... };
    };
  };
}
