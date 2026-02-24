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
        stricter. When null, the daemon uses its compiled default (0.45).
      '';
    };

    pam.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to enable Visage PAM integration. When enabled, face
        authentication is tried before password for sudo, login, and
        screen lock. Password is always available as fallback.
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
        VISAGE_CAMERA = cfg.camera;
      } // lib.optionalAttrs (cfg.similarityThreshold != null) {
        VISAGE_SIMILARITY_THRESHOLD = toString cfg.similarityThreshold;
      };

      serviceConfig = {
        Type = "simple";
        ExecStart = "${cfg.package}/bin/visaged";
        Restart = "on-failure";
        RestartSec = 5;

        # Hardening (mirrors packaging/systemd/visaged.service)
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        DeviceAllow = [ "char-video4linux rw" ];
        ReadWritePaths = [ "/var/lib/visage" ];
        CapabilityBoundingSet = "";
        SystemCallArchitectures = "native";
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
        control = "[success=end default=ignore]";
        modulePath = "${cfg.package}/lib/security/pam_visage.so";
      };
      login.rules.auth.visage = {
        order = 900;
        control = "[success=end default=ignore]";
        modulePath = "${cfg.package}/lib/security/pam_visage.so";
      };
      # Screen lockers (swaylock, hyprlock, etc.) use their own PAM service.
      # Users can add more via:
      #   security.pam.services.<name>.rules.auth.visage = { ... };
    };
  };
}
