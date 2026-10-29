use cosmic::widget;
use std::{
    collections::HashMap,
    error::Error,
    fmt::Write,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
};

use super::{Backend, Package};
use crate::{app_info::{AppIcon, AppInfo, AppKind, AppRelease}, AppId, AppstreamCache, Operation, OperationKind};

#[derive(Debug)]
pub struct Xbps {
    appstream_caches: Vec<AppstreamCache>,
}

impl Xbps {
    pub fn new(locale: &str) -> Result<Self, Box<dyn Error>> {
        let source_id = "xbps";
        let source_name = "Void Linux";
        Ok(Self {
            appstream_caches: vec![AppstreamCache::system(
                source_id.to_string(),
                source_name.to_string(),
                locale,
            )],
        })
    }
}

fn split_name_version(pkgver: &str) -> (String, String) {
    if let Some(pos) = pkgver.rfind('-') {
        let (name, ver) = pkgver.split_at(pos);
        let ver = &ver[1..];
        if !name.is_empty() && !ver.is_empty() {
            return (name.to_string(), ver.to_string());
        }
    }
    (pkgver.to_string(), String::new())
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{} {:?} failed: {}", cmd, args, stderr.trim()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_privileged_install(pkgs: &[String]) -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = vec!["-y".to_string()];
    for p in pkgs {
        args.push("--".to_string());
        args.push(p.clone());
    }
    let mut cmd_args: Vec<&str> = vec!["xbps-install"];
    for a in &args {
        cmd_args.push(a);
    }
    let status = Command::new("pkexec")
        .args(&cmd_args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(format!("pkexec xbps-install failed with {:?}", status.code()).into());
    }
    Ok(())
}

fn run_privileged_remove(pkgs: &[String]) -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = vec!["-y".to_string()];
    for p in pkgs {
        args.push("--".to_string());
        args.push(p.clone());
    }
    let mut cmd_args: Vec<&str> = vec!["xbps-remove"];
    for a in &args {
        cmd_args.push(a);
    }
    let status = Command::new("pkexec")
        .args(&cmd_args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(format!("pkexec xbps-remove failed with {:?}", status.code()).into());
    }
    Ok(())
}

fn run_privileged_update() -> Result<(), Box<dyn Error>> {
    let status = Command::new("pkexec")
        .args(["xbps-install", "-Syu"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(format!("pkexec xbps-install -Syu failed with {:?}", status.code()).into());
    }
    Ok(())
}


impl Xbps {
    fn synthesize_apps(&mut self) -> Result<(), Box<dyn Error>> {
        let out = run_cmd("xbps-query", &["-l"])?;
        let appstream_cache = &mut self.appstream_caches[0];
        let source_id = appstream_cache.source_id.clone();
        let source_name = appstream_cache.source_name.clone();
        for line in out.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let _state = parts.next();
            let pkgver = match parts.next() {
                Some(p) => p.to_string(),
                None => continue,
            };
            let (name, version) = split_name_version(&pkgver);
            // Skip if already has appstream entry
            if appstream_cache.pkgnames.contains_key(&name) {
                continue;
            }
            let app_id = AppId::new(&format!("xbps:{}", name));
            if appstream_cache.infos.contains_key(&app_id) {
                continue;
            }
            let description = parts.collect::<Vec<_>>().join(" ");
            let info = Arc::new(AppInfo {
                source_id: source_id.clone(),
                source_name: source_name.clone(),
                origin_opt: None,
                name: name.clone(),
                summary: if description.is_empty() { name.clone() } else { description.clone() },
                kind: AppKind::DesktopApplication,
                developer_name: "Void Linux".to_string(),
                description: description.clone(),
                license_opt: None,
                pkgnames: vec![name.clone()],
                package_paths: Vec::new(),
                categories: Vec::new(),
                desktop_ids: Vec::new(),
                flatpak_refs: Vec::new(),
                icons: vec![AppIcon::Stock("package-x-generic".to_string())],
                provides: Vec::new(),
                releases: vec![AppRelease {
                    timestamp: None,
                    version: version.clone(),
                    description: None,
                    url: None,
                }],
                screenshots: Vec::new(),
                urls: Vec::new(),
                monthly_downloads: 0,
            });
            appstream_cache.pkgnames.entry(name).or_default().insert(app_id.clone());
            appstream_cache.infos.insert(app_id, info);
        }
        Ok(())
    }
}

impl Backend for Xbps {
    fn load_caches(&mut self, _refresh: bool) -> Result<(), Box<dyn Error>> {
        for appstream_cache in self.appstream_caches.iter_mut() {
            appstream_cache.reload();
        }
        // Synthesize app entries for all installed xbps packages
        if let Err(e) = self.synthesize_apps() {
            log::debug!("failed to synthesize xbps apps: {}", e);
        }
        Ok(())
    }

    fn info_caches(&self) -> &[AppstreamCache] {
        &self.appstream_caches
    }

    fn installed(&self) -> Result<Vec<Package>, Box<dyn Error>> {
        let appstream_cache = &self.appstream_caches[0];
        let out = run_cmd("xbps-query", &["-l"])?;
        let mut packages = Vec::new();

        for line in out.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // xbps-query -l format: ii <pkgver> <description> ...
            let mut parts = line.split_whitespace();
            let _state = parts.next();
            let pkgver = match parts.next() {
                Some(p) => p.to_string(),
                None => continue,
            };
            let (name, version) = split_name_version(&pkgver);

            match appstream_cache.pkgnames.get(&name) {
                Some(ids) => {
                    for id in ids.iter() {
                        if let Some(info) = appstream_cache.infos.get(id) {
                            packages.push(Package {
                                id: id.clone(),
                                icon: appstream_cache.icon(info),
                                info: info.clone(),
                                version: version.clone(),
                                extra: HashMap::new(),
                            });
                        }
                    }
                }
                None => {
                    // Create synthetic entry
                    let app_id = AppId::new(&format!("xbps:{}", name));
                    if let Some(info) = appstream_cache.infos.get(&app_id) {
                        packages.push(Package {
                            id: app_id,
                            icon: appstream_cache.icon(info),
                            info: info.clone(),
                            version: version.clone(),
                            extra: HashMap::new(),
                        });
                    } else {
                        log::debug!("no app info for {}", name);
                    }
                }
            }
        }

        Ok(packages)
    }

    fn updates(&self) -> Result<Vec<Package>, Box<dyn Error>> {
        let appstream_cache = &self.appstream_caches[0];
        // Try unprivileged update check first as per handoff
        let out = match run_cmd("xbps-install", &["-nSu"]) {
            Ok(o) => o,
            Err(_) => return Ok(Vec::new()),
        };
        let mut packages = Vec::new();
        for line in out.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let pkgver = match parts.next() {
                Some(p) => p.to_string(),
                None => continue,
            };
            let (_action) = parts.next();
            let (_arch) = parts.next();
            let (_repo) = parts.next();
            let (_installed_size) = parts.next();
            let (_download_size) = parts.next();
            let (name, version) = split_name_version(&pkgver);
            match appstream_cache.pkgnames.get(&name) {
                Some(ids) => {
                    for id in ids.iter() {
                        if let Some(info) = appstream_cache.infos.get(id) {
                            packages.push(Package {
                                id: id.clone(),
                                icon: appstream_cache.icon(info),
                                info: info.clone(),
                                version: version.clone(),
                                extra: HashMap::new(),
                            });
                        }
                    }
                }
                None => {
                    let app_id = AppId::new(&format!("xbps:{}", name));
                    if let Some(info) = appstream_cache.infos.get(&app_id) {
                        packages.push(Package {
                            id: app_id,
                            icon: appstream_cache.icon(info),
                            info: info.clone(),
                            version: version.clone(),
                            extra: HashMap::new(),
                        });
                    }
                }
            }
        }
        Ok(packages)
    }

    fn file_packages(&self, _path: &str) -> Result<Vec<Package>, Box<dyn Error>> {
        Err("Xbps::file_packages not implemented".into())
    }

    fn operation(
        &self,
        op: &Operation,
        _f: Box<dyn FnMut(f32) + 'static>,
    ) -> Result<(), Box<dyn Error>> {
        match op.kind {
            OperationKind::Install => {
                let mut pkgs: Vec<String> = Vec::new();
                for info in &op.infos {
                    pkgs.extend(info.pkgnames.clone());
                }
                if pkgs.is_empty() && !op.package_ids.is_empty() {
                    for id in &op.package_ids {
                        if let Some(name) = id.raw().strip_prefix("xbps:") {
                            pkgs.push(name.to_string());
                        } else {
                            pkgs.push(id.raw().to_string());
                        }
                    }
                }
                run_privileged_install(&pkgs)
            }
            OperationKind::Uninstall { .. } => {
                let mut pkgs: Vec<String> = Vec::new();
                for info in &op.infos {
                    pkgs.extend(info.pkgnames.clone());
                }
                if pkgs.is_empty() && !op.package_ids.is_empty() {
                    for id in &op.package_ids {
                        if let Some(name) = id.raw().strip_prefix("xbps:") {
                            pkgs.push(name.to_string());
                        } else {
                            pkgs.push(id.raw().to_string());
                        }
                    }
                }
                run_privileged_remove(&pkgs)
            }
            OperationKind::Update => {
                // For system update, we update everything
                run_privileged_update()
            }
            _ => Err(format!("Xbps::operation not implemented for {:?}", op.kind).into()),
        }
    }
}
