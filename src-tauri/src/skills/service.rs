use crate::skills::api_types::{
    SkillsCatalogRequest, SkillsError, SkillsInstallRequest, SkillsProjectionRequest,
    SkillsRepairRequest, SkillsStatus, ERR_CANONICAL_ROOT_UNAVAILABLE, ERR_INVALID_PROVIDER_CONFIG,
    ERR_MANAGED_SKILL_PROTECTED, ERR_PROJECTION_DRIFT,
    ERR_PROJECTION_FALLBACK_CONFIRMATION_REQUIRED, ERR_PROJECT_NOT_REGISTERED,
    ERR_PROJECT_OUTSIDE_BOUNDARY, ERR_SKILL_NOT_FOUND, ERR_UNMANAGED_COLLISION,
};
use crate::skills::app_data::{resolve_from_base, CanonicalSkillsRoot};
use crate::skills::catalog::SkillsCatalog;
use crate::skills::catalog_store;
use crate::skills::installer;
use crate::skills::manifest::{ProjectionManifest, SkillManifest};
use crate::skills::ownership;
use crate::skills::projection::{self, ProjectionMode};
use crate::skills::provider_config::{
    expand_path, ProjectionFallbackPolicy, ProviderConfigSnapshot,
};
use crate::skills::scanner::{self, RegisteredProject};
use crate::web::project_registry::ProjectRegistry;
use parking_lot::Mutex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_PROVIDERS: &str = include_str!("../../resources/skills/providers.default.toml");

#[derive(Debug, Clone)]
pub struct SkillsHubContext {
    pub state_root: PathBuf,
    pub home: PathBuf,
    pub config_root: PathBuf,
    pub projects: Vec<RegisteredProject>,
    pub user_config: Option<String>,
    pub project_configs: Vec<(String, String)>,
}

impl SkillsHubContext {
    pub fn from_state_root(state_root: PathBuf) -> Result<Self, String> {
        let home = resolve_user_home()?;
        let config_root = home.join(".config");
        Ok(Self {
            state_root,
            home,
            config_root,
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
    }
}

#[derive(Debug, Default)]
struct LastGoodState {
    config: Option<ProviderConfigSnapshot>,
    catalog: Option<SkillsCatalog>,
    last_scan_at: Option<u64>,
    stale: bool,
}

pub struct SkillsHubService {
    root: CanonicalSkillsRoot,
    home: PathBuf,
    config_root: PathBuf,
    registry: Option<Arc<ProjectRegistry>>,
    projects_override: Vec<RegisteredProject>,
    injected_user_config: Option<String>,
    injected_project_configs: Vec<(String, String)>,
    last_good: Mutex<LastGoodState>,
}

impl SkillsHubService {
    pub fn new(context: SkillsHubContext) -> Result<Self, String> {
        let root = resolve_from_base(context.state_root.clone())?;
        Ok(Self {
            root,
            home: context.home,
            config_root: context.config_root,
            registry: None,
            projects_override: context.projects,
            injected_user_config: context.user_config,
            injected_project_configs: context.project_configs,
            last_good: Mutex::new(LastGoodState::default()),
        })
    }

    pub fn from_host(
        state_root: PathBuf,
        registry: Arc<ProjectRegistry>,
    ) -> Result<Self, SkillsError> {
        let home = resolve_user_home()
            .map_err(|error| SkillsError::new(ERR_CANONICAL_ROOT_UNAVAILABLE, error))?;
        let config_root = home.join(".config");
        let root = resolve_from_base(state_root)
            .map_err(|error| SkillsError::new(ERR_CANONICAL_ROOT_UNAVAILABLE, error))?;
        log::info!(
            target: "se_manager::skills",
            "operation=skills_hub_ready root={}",
            root.path().display()
        );
        Ok(Self {
            root,
            home,
            config_root,
            registry: Some(registry),
            projects_override: Vec::new(),
            injected_user_config: None,
            injected_project_configs: Vec::new(),
            last_good: Mutex::new(LastGoodState::default()),
        })
    }

    pub fn root(&self) -> &CanonicalSkillsRoot {
        &self.root
    }

    pub fn registered_projects(&self) -> Vec<RegisteredProject> {
        if !self.projects_override.is_empty() {
            return self.projects_override.clone();
        }
        let Some(registry) = &self.registry else {
            return Vec::new();
        };
        registry
            .snapshot()
            .projects
            .into_iter()
            .filter(|project| !project.is_archived)
            .filter_map(|project| {
                Some(RegisteredProject {
                    id: project.id,
                    root: PathBuf::from(project.path?),
                })
            })
            .collect()
    }

    pub fn resolve_request(
        &self,
        request: &SkillsCatalogRequest,
    ) -> Result<Vec<RegisteredProject>, SkillsError> {
        let registered = self.registered_projects();
        if let Some(project_id) = request
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let project = registered
                .iter()
                .find(|project| project.id == project_id)
                .cloned()
                .ok_or_else(|| {
                    SkillsError::new(
                        ERR_PROJECT_NOT_REGISTERED,
                        format!("project '{project_id}' is not registered"),
                    )
                })?;
            if let Some(project_root) = request.project_root.as_deref() {
                let requested = PathBuf::from(project_root);
                if !scanner::path_is_within(&project.root, &requested) {
                    return Err(SkillsError::coded(ERR_PROJECT_OUTSIDE_BOUNDARY));
                }
            }
            return Ok(vec![project]);
        }
        if let Some(project_root) = request
            .project_root
            .as_deref()
            .map(str::trim)
            .filter(|root| !root.is_empty())
        {
            let requested = PathBuf::from(project_root);
            let matched = registered
                .into_iter()
                .find(|project| scanner::path_is_within(&project.root, &requested));
            return matched
                .map(|project| vec![project])
                .ok_or_else(|| SkillsError::coded(ERR_PROJECT_OUTSIDE_BOUNDARY));
        }
        Ok(registered)
    }

    pub fn status(&self, request: SkillsCatalogRequest) -> Result<SkillsStatus, SkillsError> {
        let projects = self.resolve_request(&request)?;
        let persisted = catalog_store::load(&self.root.catalog_path())
            .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error))?;
        let (config, stale) = self.load_config();
        if let Some(catalog) = persisted {
            let catalog = self.filter_catalog(catalog, request.project_id.as_deref());
            return Ok(self.status_from(catalog, &config, stale, &projects));
        }
        let mut status = self.status_from(
            self.filter_catalog(SkillsCatalog::default(), request.project_id.as_deref()),
            &config,
            true,
            &projects,
        );
        status
            .catalog
            .diagnostics
            .push("SKILLS_CATALOG_UNAVAILABLE".to_string());
        Ok(status)
    }

    pub fn sync(&self, request: SkillsCatalogRequest) -> Result<SkillsStatus, SkillsError> {
        let projects = self.resolve_request(&request)?;
        let (config, used_lkg) = self.load_config();
        let mut catalog = scanner::scan(&config, &self.home, &self.config_root, &projects);
        let persisted = catalog_store::load(&self.root.catalog_path())
            .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error))?;
        catalog.revision = persisted
            .as_ref()
            .map(|previous| previous.revision.saturating_add(1))
            .unwrap_or(1);
        let now = unix_secs();
        let should_persist = !used_lkg || persisted.is_none();
        if should_persist {
            catalog_store::save_atomic(&self.root.catalog_path(), &catalog)
                .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error))?;
            self.last_good.lock().catalog = Some(catalog.clone());
        } else if let Some(previous) = persisted {
            let mut previous = previous;
            previous
                .diagnostics
                .extend(catalog.diagnostics.iter().cloned());
            catalog = previous;
        }
        self.last_good.lock().last_scan_at = Some(now);
        let catalog = self.filter_catalog(catalog, request.project_id.as_deref());
        Ok(self.status_from(catalog, &config, used_lkg, &projects))
    }

    pub fn refresh(&self) -> Result<SkillsCatalog, SkillsError> {
        Ok(self.sync(SkillsCatalogRequest::default())?.catalog)
    }

    pub fn install(&self, request: SkillsInstallRequest) -> Result<SkillManifest, SkillsError> {
        let scope = if request.scope.is_empty() {
            "global"
        } else {
            request.scope.as_str()
        };
        if !matches!(scope, "global" | "project") {
            return Err(SkillsError::new(
                ERR_PROJECT_OUTSIDE_BOUNDARY,
                "skill scope must be global or project",
            ));
        }
        let source = PathBuf::from(&request.source_path);
        if !source.is_absolute() {
            return Err(SkillsError::new(
                ERR_PROJECT_OUTSIDE_BOUNDARY,
                "install source path must be absolute",
            ));
        }
        let projects = self.resolve_request(&SkillsCatalogRequest {
            project_id: request.project_id.clone(),
            project_root: request.project_root.clone(),
        })?;
        if scope == "project" && request.project_id.is_none() && request.project_root.is_none() {
            return Err(SkillsError::coded(ERR_PROJECT_NOT_REGISTERED));
        }
        if scope == "project" && projects.is_empty() {
            return Err(SkillsError::coded(ERR_PROJECT_NOT_REGISTERED));
        }
        if self.registry.is_some()
            && !self
                .registered_projects()
                .iter()
                .any(|project| scanner::path_is_within(&project.root, &source))
            && !scanner::path_is_within(self.root.path(), &source)
        {
            return Err(SkillsError::coded(ERR_PROJECT_OUTSIDE_BOUNDARY));
        }
        if ownership::is_managed_skill(&source) {
            return Err(SkillsError::coded(ERR_MANAGED_SKILL_PROTECTED));
        }
        let mut manifest = installer::install_file(
            &self.root,
            &request.name,
            &source,
            request.confirm_token.as_deref(),
        )
        .map_err(map_install_error)?;
        let fallback = request
            .fallback
            .unwrap_or_else(|| self.load_config().0.fallback_policy);
        let project = projects.first();
        match self.project_targets(&request.name, scope, project) {
            Ok(targets) => {
                for (provider, target) in targets {
                    match self.project_one(&manifest, &provider, &target, fallback, false) {
                        Ok(projection) => manifest.projections.push(projection),
                        Err(error)
                            if error.code == ERR_PROJECTION_FALLBACK_CONFIRMATION_REQUIRED =>
                        {
                            self.save_manifest(&manifest)?;
                            return Err(error);
                        }
                        Err(error) if error.code == ERR_UNMANAGED_COLLISION => {
                            self.save_manifest(&manifest)?;
                            return Err(error);
                        }
                        Err(error) => {
                            log::warn!(
                                target: "se_manager::skills",
                                "operation=skills_project_failed code={} provider={}",
                                error.code,
                                provider
                            );
                        }
                    }
                }
            }
            Err(error) => {
                self.save_manifest(&manifest)?;
                return Err(error);
            }
        }
        self.save_manifest(&manifest)?;
        let _ = self.refresh();
        Ok(manifest)
    }

    pub fn project_skill(
        &self,
        request: SkillsProjectionRequest,
    ) -> Result<SkillManifest, SkillsError> {
        crate::skills::validate_skill_name(&request.name)
            .map_err(|_| SkillsError::coded(ERR_SKILL_NOT_FOUND))?;
        let mut manifest = self.load_manifest(&request.name)?;
        let projects = self.resolve_request(&SkillsCatalogRequest {
            project_id: request.project_id.clone(),
            project_root: request.project_root.clone(),
        })?;
        let scope = if request.project_id.is_some() || request.project_root.is_some() {
            "project"
        } else {
            "global"
        };
        let fallback = request
            .fallback
            .unwrap_or_else(|| self.load_config().0.fallback_policy);
        let confirm = request.confirm_fallback.unwrap_or(false);
        let project = projects.first();
        let targets = self.project_targets(&request.name, scope, project)?;
        for (provider, target) in targets {
            match self.project_one(&manifest, &provider, &target, fallback, confirm) {
                Ok(projection) => {
                    manifest
                        .projections
                        .retain(|existing| existing.target_path != projection.target_path);
                    manifest.projections.push(projection);
                }
                Err(error) => {
                    self.save_manifest(&manifest)?;
                    return Err(error);
                }
            }
        }
        self.save_manifest(&manifest)?;
        let _ = self.refresh();
        Ok(manifest)
    }

    pub fn repair(&self, request: SkillsRepairRequest) -> Result<SkillManifest, SkillsError> {
        crate::skills::validate_skill_name(&request.name)
            .map_err(|_| SkillsError::coded(ERR_SKILL_NOT_FOUND))?;
        let mut manifest = self.load_manifest(&request.name)?;
        if ownership::is_managed_skill(&manifest.canonical_path) {
            return Err(SkillsError::coded(ERR_MANAGED_SKILL_PROTECTED));
        }
        if manifest.projections.is_empty() {
            return Err(SkillsError::new(
                ERR_PROJECTION_DRIFT,
                "no Termul-owned projections to repair",
            ));
        }
        let fallback = self.load_config().0.fallback_policy;
        let mut repaired = Vec::new();
        let projections = std::mem::take(&mut manifest.projections);
        for projection in projections {
            if ownership::is_managed_skill(&projection.target_path) {
                return Err(SkillsError::coded(ERR_MANAGED_SKILL_PROTECTED));
            }
            let source_digest = projection::projection_digest(&manifest.canonical_path)
                .map_err(|error| SkillsError::new(ERR_PROJECTION_DRIFT, error))?;
            if projection.target_path.exists() {
                let target_digest = projection::projection_digest(&projection.target_path)
                    .map_err(|error| SkillsError::new(ERR_PROJECTION_DRIFT, error))?;
                if target_digest == source_digest {
                    repaired.push(projection);
                    continue;
                }
                if target_digest != projection.target_digest {
                    return Err(SkillsError::coded(ERR_PROJECTION_DRIFT));
                }
                let _ = fs::remove_file(&projection.target_path);
            }
            match self.project_one(
                &manifest,
                &projection.provider,
                &projection.target_path,
                fallback,
                true,
            ) {
                Ok(next) => repaired.push(next),
                Err(error) => return Err(error),
            }
        }
        manifest.projections = repaired;
        self.save_manifest(&manifest)?;
        let _ = self.refresh();
        Ok(manifest)
    }

    fn project_one(
        &self,
        manifest: &SkillManifest,
        provider: &str,
        target: &Path,
        fallback: ProjectionFallbackPolicy,
        confirm_copy: bool,
    ) -> Result<ProjectionManifest, SkillsError> {
        if ownership::is_managed_skill(target) {
            return Err(SkillsError::coded(ERR_MANAGED_SKILL_PROTECTED));
        }
        let policy = if confirm_copy && fallback == ProjectionFallbackPolicy::Ask {
            ProjectionFallbackPolicy::Copy
        } else {
            fallback
        };
        if target.exists() {
            if let Ok(existing) = projection::projection_digest(target) {
                let listed = manifest
                    .projections
                    .iter()
                    .any(|projection| projection.target_path == target);
                let linked_to_source = fs::canonicalize(target)
                    .ok()
                    .zip(fs::canonicalize(&manifest.canonical_path).ok())
                    .is_some_and(|(target, source)| target == source);
                if existing == manifest.digest && (listed || linked_to_source) {
                    return Ok(ProjectionManifest {
                        provider: provider.to_string(),
                        target_path: target.to_path_buf(),
                        mode: "present".to_string(),
                        source_digest: manifest.digest.clone(),
                        target_digest: existing,
                        fallback_reason: None,
                    });
                }
            }
            return Err(SkillsError::coded(ERR_UNMANAGED_COLLISION));
        }
        let mode = projection::project(&manifest.canonical_path, target, policy)
            .map_err(map_install_error)?;
        let target_digest = projection::projection_digest(target)
            .map_err(|error| SkillsError::new(ERR_PROJECTION_DRIFT, error))?;
        let fallback_reason = match mode {
            ProjectionMode::Copy => Some("fallback_copy".to_string()),
            ProjectionMode::Symlink => None,
        };
        Ok(ProjectionManifest {
            provider: provider.to_string(),
            target_path: target.to_path_buf(),
            mode: projection::mode_name(mode).to_string(),
            source_digest: manifest.digest.clone(),
            target_digest,
            fallback_reason,
        })
    }

    fn project_targets(
        &self,
        name: &str,
        scope: &str,
        project: Option<&RegisteredProject>,
    ) -> Result<Vec<(String, PathBuf)>, SkillsError> {
        let (config, _) = self.load_config();
        let mut targets = Vec::new();
        for provider in config.providers.iter().filter(|provider| provider.enabled) {
            let templates = if scope == "project" {
                &provider.project_roots
            } else {
                &provider.global_roots
            };
            for template in templates {
                let root = expand_path(
                    template,
                    &self.home,
                    &self.config_root,
                    project.map(|project| project.root.as_path()),
                )
                .map_err(|error| SkillsError::new(ERR_PROJECT_OUTSIDE_BOUNDARY, error))?;
                if let Some(project) = project {
                    if !scanner::path_is_within(&project.root, &root) {
                        return Err(SkillsError::coded(ERR_PROJECT_OUTSIDE_BOUNDARY));
                    }
                }
                targets.push((provider.id.clone(), root.join(name).join("SKILL.md")));
            }
        }
        Ok(targets)
    }

    fn load_config(&self) -> (ProviderConfigSnapshot, bool) {
        let layers = self.config_layers();
        let borrowed: Vec<(&str, &str)> = layers
            .iter()
            .map(|(label, raw)| (label.as_str(), raw.as_str()))
            .collect();
        let parsed = ProviderConfigSnapshot::from_layers(&borrowed);
        if parsed.has_invalid_config() {
            if let Some(lkg) = self.load_last_good_config() {
                let mut lkg = lkg;
                lkg.diagnostics.extend(parsed.diagnostics.clone());
                if !lkg
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.contains(ERR_INVALID_PROVIDER_CONFIG))
                {
                    lkg.diagnostics
                        .push(ERR_INVALID_PROVIDER_CONFIG.to_string());
                }
                self.last_good.lock().config = Some(lkg.clone());
                return (lkg, true);
            }
            self.last_good.lock().config = Some(parsed.clone());
            return (parsed, true);
        }
        if let Ok(serialized) = toml::to_string_pretty(&parsed.to_file()) {
            let path = self.last_good_config_path();
            let unchanged = fs::read_to_string(&path).is_ok_and(|existing| existing == serialized);
            if !unchanged {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(path, serialized);
            }
        }
        self.last_good.lock().config = Some(parsed.clone());
        (parsed, false)
    }

    fn load_last_good_config(&self) -> Option<ProviderConfigSnapshot> {
        if let Some(config) = self.last_good.lock().config.clone() {
            if !config.providers.is_empty() {
                return Some(config);
            }
        }
        let raw = fs::read_to_string(self.last_good_config_path()).ok()?;
        let snapshot = ProviderConfigSnapshot::from_layers(&[("last-good", raw.as_str())]);
        if snapshot.providers.is_empty() {
            None
        } else {
            Some(snapshot)
        }
    }

    fn config_layers(&self) -> Vec<(String, String)> {
        let mut layers = vec![("default".to_string(), DEFAULT_PROVIDERS.to_string())];
        let host_config = self.root.path().join("agent-providers.toml");
        if let Ok(raw) = fs::read_to_string(host_config) {
            layers.push(("host".to_string(), raw));
        }
        if let Some(raw) = &self.injected_user_config {
            layers.push(("user".to_string(), raw.clone()));
        } else {
            let user_config = self.home.join(".se-manager").join("agent-providers.toml");
            if let Ok(raw) = fs::read_to_string(user_config) {
                layers.push(("user".to_string(), raw));
            }
        }
        if !self.injected_project_configs.is_empty() {
            layers.extend(self.injected_project_configs.clone());
        } else {
            for project in self.registered_projects() {
                let path = project
                    .root
                    .join(".se-manager")
                    .join("agent-providers.toml");
                if let Ok(raw) = fs::read_to_string(path) {
                    layers.push((format!("project:{}", project.id), raw));
                }
            }
        }
        layers
    }

    fn last_good_config_path(&self) -> PathBuf {
        self.root.path().join("providers.last-good.toml")
    }

    fn filter_catalog(
        &self,
        mut catalog: SkillsCatalog,
        project_id: Option<&str>,
    ) -> SkillsCatalog {
        if let Some(project_id) = project_id {
            catalog.skills.retain(|skill| {
                skill.scope == "global" || skill.project_id.as_deref() == Some(project_id)
            });
        }
        catalog
    }

    fn status_from(
        &self,
        catalog: SkillsCatalog,
        config: &ProviderConfigSnapshot,
        stale: bool,
        projects: &[RegisteredProject],
    ) -> SkillsStatus {
        let watched_roots = self.watched_roots(config, projects);
        let last_good = self.last_good.lock();
        SkillsStatus {
            watched_roots,
            last_scan_at: last_good.last_scan_at,
            catalog,
            fallback_policy: config.fallback_policy,
            stale: stale || last_good.stale,
        }
    }

    fn watched_roots(
        &self,
        config: &ProviderConfigSnapshot,
        projects: &[RegisteredProject],
    ) -> Vec<String> {
        let mut roots = vec![self.root.path().to_string_lossy().into_owned()];
        for provider in config.providers.iter().filter(|provider| provider.enabled) {
            for template in &provider.global_roots {
                if let Ok(path) = expand_path(template, &self.home, &self.config_root, None) {
                    if path.is_dir() {
                        roots.push(path.to_string_lossy().into_owned());
                    }
                }
            }
            for project in projects {
                for template in &provider.project_roots {
                    if let Ok(path) =
                        expand_path(template, &self.home, &self.config_root, Some(&project.root))
                    {
                        if path.is_dir() && scanner::path_is_within(&project.root, &path) {
                            roots.push(path.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }

    fn manifest_path(&self, name: &str) -> Result<PathBuf, SkillsError> {
        crate::skills::validate_skill_name(name)
            .map_err(|_| SkillsError::coded(ERR_SKILL_NOT_FOUND))?;
        Ok(self.root.manifests_dir().join(format!("{name}.json")))
    }

    fn save_manifest(&self, manifest: &SkillManifest) -> Result<(), SkillsError> {
        let path = self.manifest_path(&manifest.name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error.to_string()))?;
        }
        let body = serde_json::to_vec_pretty(manifest)
            .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error.to_string()))?;
        fs::write(path, body)
            .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error.to_string()))
    }

    fn load_manifest(&self, name: &str) -> Result<SkillManifest, SkillsError> {
        let path = self.manifest_path(name)?;
        let raw = fs::read_to_string(&path).map_err(|_| SkillsError::coded(ERR_SKILL_NOT_FOUND))?;
        serde_json::from_str(&raw)
            .map_err(|error| SkillsError::new("SKILLS_STATUS_ERROR", error.to_string()))
    }

    pub fn run_watch_loop(
        this: Arc<Self>,
        emit: impl Fn(crate::skills::api_types::SkillsHubEvent) + Send + 'static,
    ) {
        use crate::skills::api_types::{
            SkillsHubEvent, EVENT_CATALOG_CHANGED, EVENT_CONFLICT_DETECTED, EVENT_LINK_DRIFT,
            EVENT_SYNC_STALE,
        };
        use crate::skills::watcher::{relevant_path, SkillsWatchEvent, SkillsWatcher};
        loop {
            let roots: Vec<PathBuf> = this
                .status_from(
                    SkillsCatalog::default(),
                    &this.load_config().0,
                    false,
                    &this.registered_projects(),
                )
                .watched_roots
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let watcher = match SkillsWatcher::start(&roots) {
                Ok(watcher) => {
                    log::info!(
                        target: "se_manager::skills",
                        "operation=skills_watch_started roots={}",
                        roots.len()
                    );
                    watcher
                }
                Err(error) => {
                    log::warn!(
                        target: "se_manager::skills",
                        "operation=skills_watch_start_failed error={error}"
                    );
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
            };
            let mut last_roots_check = Instant::now();
            loop {
                if last_roots_check.elapsed() >= Duration::from_secs(1) {
                    let current_roots = this
                        .status_from(
                            SkillsCatalog::default(),
                            &this.load_config().0,
                            false,
                            &this.registered_projects(),
                        )
                        .watched_roots;
                    let watched_root_strings: Vec<String> = roots
                        .iter()
                        .map(|root| root.to_string_lossy().into_owned())
                        .collect();
                    if current_roots != watched_root_strings {
                        break;
                    }
                    last_roots_check = Instant::now();
                }
                match watcher.try_next() {
                    Some(SkillsWatchEvent::Stale { reason }) => {
                        this.last_good.lock().stale = true;
                        emit(SkillsHubEvent {
                            kind: EVENT_SYNC_STALE.to_string(),
                            revision: 0,
                            project_id: None,
                            paths: vec![reason],
                        });
                        let _ = this.sync(SkillsCatalogRequest::default());
                        break;
                    }
                    Some(SkillsWatchEvent::Changed { paths }) => {
                        let relevant: Vec<String> = paths
                            .iter()
                            .filter(|path| roots.iter().any(|root| relevant_path(root, path)))
                            .filter(|path| !this.is_internal_path(path))
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect();
                        if relevant.is_empty() {
                            continue;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(75));
                        while watcher.try_next().is_some() {}
                        if let Ok(status) = this.sync(SkillsCatalogRequest::default()) {
                            this.last_good.lock().stale = false;
                            emit(SkillsHubEvent {
                                kind: EVENT_CATALOG_CHANGED.to_string(),
                                revision: status.catalog.revision,
                                project_id: None,
                                paths: relevant,
                            });
                            if status.catalog.skills.iter().any(|skill| skill.conflict) {
                                emit(SkillsHubEvent {
                                    kind: EVENT_CONFLICT_DETECTED.to_string(),
                                    revision: status.catalog.revision,
                                    project_id: None,
                                    paths: Vec::new(),
                                });
                            }
                            if status.catalog.skills.iter().any(|skill| skill.drift) {
                                emit(SkillsHubEvent {
                                    kind: EVENT_LINK_DRIFT.to_string(),
                                    revision: status.catalog.revision,
                                    project_id: None,
                                    paths: Vec::new(),
                                });
                            }
                        }
                    }
                    Some(SkillsWatchEvent::FullRescan) => {
                        let _ = this.sync(SkillsCatalogRequest::default());
                    }
                    None => std::thread::sleep(std::time::Duration::from_millis(50)),
                }
            }
        }
    }

    fn is_internal_path(&self, path: &Path) -> bool {
        let is_catalog_file = path == self.root.catalog_path()
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("catalog.json.tmp-"));
        is_catalog_file
            || path == self.root.path()
            || path == self.last_good_config_path()
            || scanner::path_is_within(&self.root.manifests_dir(), path)
    }

    pub fn default_state_root() -> Result<PathBuf, String> {
        crate::web::config::default_sessions_dir()
            .and_then(|sessions| sessions.parent().map(PathBuf::from))
            .ok_or_else(|| {
                format!("{ERR_CANONICAL_ROOT_UNAVAILABLE}: could not resolve host state root")
            })
    }
}

fn resolve_user_home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "could not resolve user home directory".to_string())
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn map_install_error(error: String) -> SkillsError {
    let code = if error.starts_with(ERR_UNMANAGED_COLLISION) {
        ERR_UNMANAGED_COLLISION
    } else if error.starts_with(ERR_MANAGED_SKILL_PROTECTED) {
        ERR_MANAGED_SKILL_PROTECTED
    } else if error.starts_with(ERR_PROJECTION_FALLBACK_CONFIRMATION_REQUIRED) {
        ERR_PROJECTION_FALLBACK_CONFIRMATION_REQUIRED
    } else if error.starts_with("PROJECTION_UNSUPPORTED") {
        "PROJECTION_UNSUPPORTED"
    } else if error.starts_with("INSTALL_DIGEST_MISMATCH") {
        "INSTALL_DIGEST_MISMATCH"
    } else {
        "SKILLS_STATUS_ERROR"
    };
    SkillsError::new(code, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_skill(dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name}\n---\n{body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn status_scans_real_default_global_provider_root_and_persists() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write_skill(&home.join(".agents/skills/demo"), "demo", "body");
        let context = SkillsHubContext {
            state_root: temp.path().join("state"),
            home,
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        };
        let service = SkillsHubService::new(context).unwrap();
        let status = service.sync(SkillsCatalogRequest::default()).unwrap();
        assert!(status
            .catalog
            .skills
            .iter()
            .any(|skill| skill.name == "demo"));
        assert!(service.root().catalog_path().is_file());
        assert!(!status.watched_roots.is_empty());
    }

    #[test]
    fn invalid_provider_config_keeps_last_known_good_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write_skill(&home.join(".agents/skills/demo"), "demo", "body");
        let context = SkillsHubContext {
            state_root: temp.path().join("state"),
            home: home.clone(),
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        };
        let service = SkillsHubService::new(context).unwrap();
        service.sync(SkillsCatalogRequest::default()).unwrap();
        fs::create_dir_all(service.root().path()).unwrap();
        fs::write(
            service.root().path().join("agent-providers.toml"),
            "not toml",
        )
        .unwrap();
        let status = service.sync(SkillsCatalogRequest::default()).unwrap();
        assert!(status.stale);
        assert!(status
            .catalog
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains(ERR_INVALID_PROVIDER_CONFIG)));
        assert!(status
            .catalog
            .skills
            .iter()
            .any(|skill| skill.name == "demo"));
    }

    #[test]
    fn unregistered_project_id_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let service = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("state"),
            home: temp.path().join("home"),
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        let error = service
            .status(SkillsCatalogRequest {
                project_id: Some("missing".into()),
                project_root: None,
            })
            .unwrap_err();
        assert_eq!(error.code, ERR_PROJECT_NOT_REGISTERED);
    }

    #[test]
    fn legacy_project_root_outside_registry_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let service = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("state"),
            home: temp.path().join("home"),
            config_root: temp.path().join("config"),
            projects: vec![RegisteredProject {
                id: "p1".into(),
                root: project,
            }],
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        let error = service
            .status(SkillsCatalogRequest {
                project_id: None,
                project_root: Some(temp.path().join("outside").to_string_lossy().into_owned()),
            })
            .unwrap_err();
        assert_eq!(error.code, ERR_PROJECT_OUTSIDE_BOUNDARY);
    }

    #[test]
    fn independent_host_roots_do_not_share_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write_skill(&home.join(".agents/skills/demo"), "demo", "body");
        let left = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("desktop"),
            home: home.clone(),
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        let right = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("standalone"),
            home,
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        left.sync(SkillsCatalogRequest::default()).unwrap();
        assert!(left.root().catalog_path().is_file());
        assert!(!right.root().catalog_path().is_file());
        assert_ne!(left.root().path(), right.root().path());
    }

    #[test]
    fn sync_increments_catalog_revision() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write_skill(&home.join(".agents/skills/demo"), "demo", "body");
        let service = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("state"),
            home,
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        let first = service.sync(SkillsCatalogRequest::default()).unwrap();
        let second = service.sync(SkillsCatalogRequest::default()).unwrap();
        assert!(second.catalog.revision > first.catalog.revision);
    }

    #[test]
    fn host_install_rejects_lexical_escape_from_canonical_root() {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("state");
        let source = state_root.join("skills-hub/../outside/SKILL.md");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "---\nname: demo\n---\nbody\n").unwrap();
        let registry = Arc::new(ProjectRegistry::new());
        let service = SkillsHubService::from_host(state_root, registry).unwrap();
        let error = service
            .install(SkillsInstallRequest {
                name: "demo".into(),
                source_path: source.to_string_lossy().into_owned(),
                scope: "global".into(),
                project_id: None,
                project_root: None,
                confirm_token: None,
                fallback: Some(ProjectionFallbackPolicy::Deny),
            })
            .unwrap_err();
        assert_eq!(error.code, ERR_PROJECT_OUTSIDE_BOUNDARY);
    }

    #[test]
    fn watcher_ignores_catalog_temp_and_manifest_paths() {
        let temp = tempfile::tempdir().unwrap();
        let service = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("state"),
            home: temp.path().join("home"),
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        assert!(service.is_internal_path(&service.root.catalog_path()));
        assert!(service.is_internal_path(&service.root.path().join("catalog.json.tmp-123")));
        assert!(service.is_internal_path(&service.root.manifests_dir().join("demo.json")));
        assert!(!service.is_internal_path(&service.root.canonical_dir().join("demo/SKILL.md")));
    }

    #[test]
    fn project_and_repair_reject_traversal_names_before_manifest_access() {
        let temp = tempfile::tempdir().unwrap();
        let service = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("state"),
            home: temp.path().join("home"),
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: None,
            project_configs: Vec::new(),
        })
        .unwrap();
        let project_error = service
            .project_skill(SkillsProjectionRequest {
                name: "../escape".into(),
                project_id: None,
                project_root: None,
                confirm_fallback: None,
                fallback: None,
            })
            .unwrap_err();
        assert_eq!(project_error.code, ERR_SKILL_NOT_FOUND);
        let repair_error = service
            .repair(SkillsRepairRequest {
                name: "../escape".into(),
                project_id: None,
                project_root: None,
            })
            .unwrap_err();
        assert_eq!(repair_error.code, ERR_SKILL_NOT_FOUND);
    }

    #[test]
    fn install_is_idempotent_for_same_digest() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let source_dir = home.join("src/demo");
        write_skill(&source_dir, "demo", "body");
        let service = SkillsHubService::new(SkillsHubContext {
            state_root: temp.path().join("state"),
            home,
            config_root: temp.path().join("config"),
            projects: Vec::new(),
            user_config: Some(
                r#"fallback_policy = "deny"
[providers.agents]
enabled = false
"#
                .into(),
            ),
            project_configs: Vec::new(),
        })
        .unwrap();
        let request = SkillsInstallRequest {
            name: "demo".into(),
            source_path: source_dir.join("SKILL.md").to_string_lossy().into_owned(),
            scope: "global".into(),
            project_id: None,
            project_root: None,
            confirm_token: None,
            fallback: Some(ProjectionFallbackPolicy::Deny),
        };
        let first = service.install(request.clone()).unwrap();
        let second = service.install(request).unwrap();
        assert_eq!(first.digest, second.digest);
    }
}
