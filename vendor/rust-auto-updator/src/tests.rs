mod regression {
    use crate::install::{
        Plan, cleanup, new_stage, replace, validate_stage, wait_for_exit, wait_for_parent,
    };
    use crate::verify::{checksum, hash, verify_executable};
    use crate::{Architecture, Config, parse_release};
    use std::{fs, path::PathBuf};

    fn config() -> Config {
        Config::new("example", "sample-app", "0.2.0", "SampleApp").unwrap()
    }
    fn test_parent() -> PathBuf {
        let path = std::env::current_dir().unwrap().join("target");
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn parse(json: &str, current: &str) -> Result<Option<crate::Release>, String> {
        let mut config = config();
        config.current_version = current.parse().unwrap();
        parse_release(json, &config)
    }
    fn release_json(tag: &str) -> serde_json::Value {
        let repo = config().repository_url();
        let executable = format!("SampleApp-{tag}.exe");
        serde_json::json!({"tag_name":tag,"html_url":format!("{repo}/releases/tag/{tag}"),"draft":false,"prerelease":false,
            "assets":[{"name":executable,"browser_download_url":format!("{repo}/releases/download/{tag}/{executable}"),"size":4096},
            {"name":"SHA256SUMS.txt","browser_download_url":format!("{repo}/releases/download/{tag}/SHA256SUMS.txt"),"size":93}]})
    }

    fn executable_fixture(architecture: Architecture) -> Vec<u8> {
        let mut bytes = vec![0u8; 1024];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&architecture.machine().to_le_bytes());
        bytes[70..72].copy_from_slice(&1u16.to_le_bytes());
        bytes[84..86].copy_from_slice(&112u16.to_le_bytes());
        bytes[86..88].copy_from_slice(&2u16.to_le_bytes());
        let magic: u16 = if architecture == Architecture::X86 {
            0x010b
        } else {
            0x020b
        };
        bytes[88..90].copy_from_slice(&magic.to_le_bytes());
        bytes[104..108].copy_from_slice(&4096u32.to_le_bytes());
        bytes[144..148].copy_from_slice(&8192u32.to_le_bytes());
        bytes[148..152].copy_from_slice(&512u32.to_le_bytes());
        bytes[156..158].copy_from_slice(&2u16.to_le_bytes());
        bytes[208..212].copy_from_slice(&512u32.to_le_bytes());
        bytes[212..216].copy_from_slice(&4096u32.to_le_bytes());
        bytes[216..220].copy_from_slice(&512u32.to_le_bytes());
        bytes[220..224].copy_from_slice(&512u32.to_le_bytes());
        bytes[236..240].copy_from_slice(&0x20000000u32.to_le_bytes());
        bytes
    }

    #[test]
    fn legacy_filenames_and_per_asset_checksums_are_explicit_and_repo_bound() {
        let mut config = config();
        config.asset_suffixes = vec!["-x64".into()];
        config.legacy_executable_names = vec!["sample-app-{version}-windows-x64.exe".into()];
        config.checksum_name = "{executable}.sha256".into();
        let mut json = release_json("v0.3.0");
        let name = "sample-app-0.3.0-windows-x64.exe";
        let root = format!("{}/releases/download/v0.3.0", config.repository_url());
        json["assets"][0]["name"] = name.into();
        json["assets"][0]["browser_download_url"] = format!("{root}/{name}").into();
        json["assets"][1]["name"] = format!("{name}.sha256").into();
        json["assets"][1]["browser_download_url"] = format!("{root}/{name}.sha256").into();
        let release = parse_release(&json.to_string(), &config).unwrap().unwrap();
        assert_eq!(release.executable_name(), name);
        config.legacy_executable_names.clear();
        assert!(parse_release(&json.to_string(), &config).is_err());
        config.legacy_executable_names = vec!["../escape.exe".into()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn digest_fallback_is_opt_in_and_rejects_missing_or_malformed_integrity() {
        let mut config = config();
        let mut json = release_json("v0.3.0");
        json["assets"].as_array_mut().unwrap().pop();
        json["assets"][0]["digest"] = format!("sha256:{}", "ab".repeat(32)).into();
        assert!(parse_release(&json.to_string(), &config).is_err());
        config.allow_asset_digest = true;
        let release = parse_release(&json.to_string(), &config).unwrap().unwrap();
        assert!(release.checksum_url().is_none());
        assert_eq!(release.sha256_digest(), Some("ab".repeat(32).as_str()));
        json["assets"][0]["digest"] = "sha256:invalid".into();
        assert!(parse_release(&json.to_string(), &config).is_err());
        json["assets"][0]["digest"] = serde_json::Value::Null;
        assert!(parse_release(&json.to_string(), &config).is_err());
        let mut json = release_json("v0.3.0");
        json["assets"][0]["digest"] = "sha256:invalid".into();
        assert!(parse_release(&json.to_string(), &config).is_err());
        json["assets"][0]["digest"] = format!("sha256:{}", "ab".repeat(32)).into();
        json["assets"][1]["browser_download_url"] = "https://example.com/sums".into();
        assert!(parse_release(&json.to_string(), &config).is_err());
    }

    #[test]
    fn build_metadata_never_causes_reinstallation_and_cancelled_prepare_makes_no_request() {
        assert!(
            parse(&release_json("v0.3.0+build.2").to_string(), "0.3.0+build.1")
                .unwrap()
                .is_none()
        );
        let config = config();
        let release = parse_release(&release_json("v0.3.0").to_string(), &config)
            .unwrap()
            .unwrap();
        let result = crate::prepare_download_cancellable(
            &release,
            &config,
            &std::sync::atomic::AtomicBool::new(true),
            |_, _| panic!("cancelled update cannot report progress"),
        );
        assert!(result.err().unwrap().contains("cancelled"));
    }

    #[test]
    fn startup_health_child_fixture() {
        if std::env::var_os("RUST_UPDATOR_HEALTH_TEST_SLEEP").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    #[test]
    fn startup_health_preserves_live_child_on_timeout_and_accepts_ack_before_exit() {
        use std::{os::windows::process::CommandExt, process::Command, time::Duration};
        let directory = new_stage(&test_parent(), &config().install).unwrap();
        let launch = |sleep: bool| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "tests::regression::startup_health_child_fixture"])
                .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if sleep {
                command.env("RUST_UPDATOR_HEALTH_TEST_SLEEP", "1");
            }
            command.spawn().unwrap()
        };
        let expected = "ab".repeat(32);
        let mut child = launch(true);
        crate::install::wait_for_startup_health(&mut child, &directory, &expected, Duration::ZERO)
            .unwrap();
        assert!(
            child.try_wait().unwrap().is_none(),
            "a timeout cannot terminate a live child"
        );
        assert!(directory.join("health-timeout.txt").exists());
        child.wait().unwrap();
        fs::remove_file(directory.join("health-timeout.txt")).unwrap();
        let mut child = launch(false);
        child.wait().unwrap();
        fs::write(directory.join("healthy"), &expected).unwrap();
        crate::install::wait_for_startup_health(&mut child, &directory, &expected, Duration::ZERO)
            .unwrap();
        assert_eq!(
            fs::read(directory.join("health-accepted")).unwrap(),
            expected.as_bytes()
        );
        fs::remove_file(directory.join("health-accepted")).unwrap();
        fs::remove_file(directory.join("healthy")).unwrap();
        assert!(
            crate::install::wait_for_startup_health(
                &mut child,
                &directory,
                &expected,
                Duration::ZERO
            )
            .is_err()
        );
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn a_running_process_cannot_be_treated_as_exited() {
        let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
        assert!(
            wait_for_exit(process, 0)
                .unwrap_err()
                .contains("did not exit")
        );
    }

    #[test]
    fn a_queryable_process_must_match_the_update_target() {
        let plan = Plan {
            target: std::env::current_dir().unwrap().join("Cargo.toml"),
            parent_pid: std::process::id(),
            original_sha: String::new(),
            new_sha: String::new(),
            version: config().current_version.to_string(),
            restart_args: None,
            rollback_args: None,
            smoke_marker: None,
            smoke_fail_before_health: None,
        };
        assert!(
            wait_for_parent(&plan)
                .unwrap_err()
                .contains("does not match the target executable")
        );
    }

    #[test]
    fn stable_release_selection_is_semantic_and_repo_bound() {
        assert!(
            parse(&release_json("v0.10.0").to_string(), "0.9.0")
                .unwrap()
                .is_some()
        );
        assert!(
            parse(&release_json("v0.1.2").to_string(), "0.2.0")
                .unwrap()
                .is_none()
        );
        let mut json = release_json("v9.0.0");
        json["assets"][0]["browser_download_url"] = "https://example.com/tool.exe".into();
        assert!(parse(&json.to_string(), "0.2.0").is_err());
        json = release_json("v9.0.0");
        json["assets"][0]["name"] = "SampleApp-v8.0.0.exe".into();
        assert!(parse(&json.to_string(), "0.2.0").is_err());
        json = release_json("v9.0.0");
        json["assets"][0]["name"] = "update-helper.exe".into();
        assert!(parse(&json.to_string(), "0.2.0").is_err());
        json = release_json("v9.0.0");
        json["prerelease"] = true.into();
        assert!(parse(&json.to_string(), "0.2.0").unwrap().is_none());
    }
    #[test]
    fn another_product_and_architecture_use_only_the_configured_asset() {
        let mut config =
            Config::new("another-owner", "another-repo", "1.9.0", "AnotherTool").unwrap();
        config.asset_suffix = "-arm64".into();
        config.architecture = Architecture::Arm64;
        config.checksum_name = "checksums.txt".into();
        let repo = config.repository_url();
        let name = "AnotherTool-v1.10.0-arm64.exe";
        let json = serde_json::json!({
            "tag_name":"v1.10.0", "html_url":format!("{repo}/releases/tag/v1.10.0"),
            "draft":false, "prerelease":false,
            "assets":[
                {"name":name, "size":4096, "browser_download_url":format!("{repo}/releases/download/v1.10.0/{name}")},
                {"name":"checksums.txt", "size":93, "browser_download_url":format!("{repo}/releases/download/v1.10.0/checksums.txt")}
            ]
        });
        let release = parse_release(&json.to_string(), &config).unwrap().unwrap();
        assert_eq!(release.version.to_string(), "1.10.0");
        config.asset_suffix.clear();
        assert!(parse_release(&json.to_string(), &config).is_err());
        config.asset_suffix = "-arm64".into();
        config.owner = "unexpected-owner".into();
        assert!(parse_release(&json.to_string(), &config).is_err());
    }

    #[test]
    fn checksum_is_bound_to_the_versioned_asset_and_rejects_ambiguous_entries() {
        let hash = "ab".repeat(32);
        let name = "SampleApp-v0.2.1.exe";
        assert_eq!(checksum(&format!("{hash}  {name}\n"), name).unwrap(), hash);
        assert!(checksum("no checksum", name).is_err());
        assert!(checksum(&format!("{hash} {name}\n{hash} {name}"), name).is_err());
        assert!(checksum(&format!("{} {name}", "gg".repeat(32)), name).is_err());
        assert!(checksum(&format!("{hash} SampleApp-v0.2.0.exe"), name).is_err());
        assert!(checksum(&format!("{hash} update-helper.exe"), name).is_err());
        assert!(checksum(&format!("{hash} {name} trailing"), name).is_err());
        assert_eq!(crate::single_asset_checksum(&hash, name).unwrap(), hash);
        assert!(checksum(&hash, name).is_err());
        assert!(crate::single_asset_checksum(&format!("{hash}\n{hash}"), name).is_err());
    }
    #[test]
    fn replacement_keeps_a_recoverable_backup_and_does_not_touch_preferences() {
        let parent = test_parent();
        let directory = new_stage(&parent, &config().install).unwrap();
        let target = directory.join("installed.exe");
        let ready = directory.join("next.exe");
        let backup = directory.join("backup.exe");
        fs::write(&target, b"old binary").unwrap();
        fs::write(&ready, b"new binary").unwrap();
        fs::write(directory.join("settings.json"), b"user preferences").unwrap();
        replace(&target, &ready, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new binary");
        assert_eq!(fs::read(&backup).unwrap(), b"old binary");
        assert_eq!(
            fs::read(directory.join("settings.json")).unwrap(),
            b"user preferences"
        );
        for name in ["installed.exe", "backup.exe", "settings.json"] {
            fs::remove_file(directory.join(name)).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn failed_restart_restores_previous_binary_and_keeps_preferences() {
        let directory = new_stage(&test_parent(), &config().install).unwrap();
        let target = directory.join("installed.exe");
        let ready = directory.join("ready.exe");
        let backup = directory.join("previous.exe");
        fs::write(&target, b"old executable").unwrap();
        fs::write(&ready, b"new executable").unwrap();
        fs::write(directory.join("settings.json"), b"preferences").unwrap();
        let mut restarted_old = false;
        let result = crate::install::finish_replacement(
            &target,
            &ready,
            &backup,
            &directory,
            || Err("injected restart failure".into()),
            || {
                restarted_old = true;
            },
        );
        assert!(
            result
                .unwrap_err()
                .contains("previous version was restored")
        );
        assert!(restarted_old);
        assert_eq!(fs::read(&target).unwrap(), b"old executable");
        assert_eq!(fs::read(&backup).unwrap(), b"old executable");
        assert_eq!(
            fs::read(directory.join("failed.exe")).unwrap(),
            b"new executable"
        );
        assert_eq!(
            fs::read(directory.join("settings.json")).unwrap(),
            b"preferences"
        );
        for name in [
            "installed.exe",
            "previous.exe",
            "failed.exe",
            "settings.json",
        ] {
            fs::remove_file(directory.join(name)).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn executable_validation_rejects_corruption_truncation_and_wrong_architecture() {
        let directory = new_stage(&test_parent(), &config().install).unwrap();
        let path = directory.join("update-helper.exe");
        let mut bytes = executable_fixture(Architecture::X64);
        fs::write(&path, &bytes).unwrap();
        let expected = hash(&path).unwrap();
        assert!(verify_executable(&path, &expected, 1024, Architecture::X64).is_ok());
        bytes[900] = 1;
        fs::write(&path, &bytes).unwrap();
        assert!(verify_executable(&path, &expected, 1024, Architecture::X64).is_err());
        assert!(verify_executable(&path, &hash(&path).unwrap(), 2048, Architecture::X64).is_err());
        bytes[68..70].copy_from_slice(&0x14cu16.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        assert!(verify_executable(&path, &hash(&path).unwrap(), 1024, Architecture::X64).is_err());
        bytes[60..64].copy_from_slice(&2048u32.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        assert!(verify_executable(&path, &hash(&path).unwrap(), 1024, Architecture::X64).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn complete_pe_validation_rejects_dll_truncated_sections_and_unbacked_entry() {
        let directory = new_stage(&test_parent(), &config().install).unwrap();
        let path = directory.join("fixture.exe");
        for architecture in [Architecture::X64, Architecture::X86, Architecture::Arm64] {
            fs::write(&path, executable_fixture(architecture)).unwrap();
            crate::verify_pe(&path, architecture).unwrap();
        }
        for (offset, value) in [
            (86, 0x2002u32),
            (216, 1024u32),
            (220, 128u32),
            (236, 0u32),
            (104, 6000u32),
        ] {
            let mut bytes = executable_fixture(Architecture::X64);
            if offset == 86 {
                bytes[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes());
            } else {
                bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            fs::write(&path, bytes).unwrap();
            assert!(
                crate::verify_pe(&path, Architecture::X64).is_err(),
                "offset {offset}"
            );
        }
        let mut bytes = executable_fixture(Architecture::X64);
        bytes[156..158].copy_from_slice(&3u16.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        crate::verify_pe(&path, Architecture::X64).unwrap();
        assert!(crate::verify_pe_subsystem(&path, Architecture::X64, Some(2)).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn cleanup_rejects_non_staging_paths_and_preserves_unknown_files() {
        let parent = test_parent();
        assert!(validate_stage(&parent, &parent, &config().install).is_err());
        let directory = new_stage(&parent, &config().install).unwrap();
        assert!(validate_stage(&directory, &parent.join("debug"), &config().install).is_err());
        fs::write(directory.join("update-helper.exe"), b"temporary executable").unwrap();
        fs::write(directory.join("keep.txt"), b"unrelated data").unwrap();
        cleanup(&directory, &parent, &config().install);
        assert!(!directory.join("update-helper.exe").exists());
        assert_eq!(
            fs::read(directory.join("keep.txt")).unwrap(),
            b"unrelated data"
        );
        fs::remove_file(directory.join("keep.txt")).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
