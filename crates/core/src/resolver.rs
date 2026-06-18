use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use oxc_resolver::{ResolveOptions, Resolver as OxcResolver};

#[derive(Clone)]
pub(crate) struct Resolver {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    entry_path: String,
    resolver: OxcResolver,
}

impl Resolver {
    pub(crate) fn new(entry: &Path, module_root: Option<&Path>) -> Result<Self> {
        let entry = entry
            .canonicalize()
            .with_context(|| format!("failed to resolve JS entry path {}", entry.display()))?;

        if !entry.is_file() {
            return Err(anyhow!("JS entry path is not a file: {}", entry.display()));
        }

        let root = match module_root {
            Some(root) => root
                .canonicalize()
                .with_context(|| format!("failed to resolve module root {}", root.display()))?,
            None => default_module_root(&entry)?,
        };

        if !root.is_dir() {
            return Err(anyhow!(
                "module root is not a directory: {}",
                root.display()
            ));
        }

        let relative_entry = entry.strip_prefix(&root).with_context(|| {
            format!(
                "JS entry path {} is not under module root {}",
                entry.display(),
                root.display()
            )
        })?;

        let entry_path = guest_absolute_path(relative_entry)?;
        let resolver = OxcResolver::new(ResolveOptions {
            condition_names: vec!["import".into(), "default".into()],
            extensions: vec![".mjs".into(), ".js".into()],
            main_fields: vec!["module".into(), "main".into()],
            node_path: false,
            symlinks: false,
            ..ResolveOptions::default()
        });

        Ok(Self {
            inner: Arc::new(Inner {
                root,
                entry_path,
                resolver,
            }),
        })
    }

    pub(crate) fn resolve(&self, referrer: &str, specifier: &str) -> Result<String> {
        let referrer = self.guest_path_to_host(referrer)?;
        let resolved = self
            .inner
            .resolver
            .resolve_file(&referrer, specifier)
            .with_context(|| {
                format!(
                    "filesystem module not found: failed to resolve JavaScript import {specifier:?} from {}",
                    referrer.display()
                )
            })?
            .path()
            .canonicalize()
            .with_context(|| {
                format!(
                    "failed to canonicalize JavaScript import {specifier:?} from {}",
                    referrer.display()
                )
            })?;
        let relative = resolved.strip_prefix(&self.inner.root).with_context(|| {
            format!(
                "resolved JavaScript module {} is not under module root {}",
                resolved.display(),
                self.inner.root.display()
            )
        })?;

        guest_absolute_path(relative)
    }

    pub(crate) fn load(&self, path: &str) -> Result<String> {
        let path = self.guest_path_to_host(path)?;
        std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read JavaScript module {}", path.display()))
    }

    fn guest_path_to_host(&self, path: &str) -> Result<PathBuf> {
        let path = path
            .strip_prefix('/')
            .ok_or_else(|| anyhow!("JavaScript module path must be absolute: {path}"))?;
        let path = self.inner.root.join(path);
        let path = path
            .canonicalize()
            .with_context(|| format!("failed to resolve JavaScript module {}", path.display()))?;
        if !path.starts_with(&self.inner.root) {
            anyhow::bail!(
                "JavaScript module path {} escapes module root {}",
                path.display(),
                self.inner.root.display()
            );
        }
        Ok(path)
    }

    pub(crate) fn entry_path(&self) -> &str {
        &self.inner.entry_path
    }
}

fn default_module_root(entry: &Path) -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .context("failed to read current directory")?
        .canonicalize()
        .context("failed to resolve current directory")?;

    if entry.starts_with(&cwd) {
        return Ok(cwd);
    }

    entry
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("JS entry path has no parent: {}", entry.display()))
}

fn guest_absolute_path(relative: &Path) -> Result<String> {
    let mut guest = String::from("/");
    let mut first = true;

    for component in relative.components() {
        let PathComponent::Normal(part) = component else {
            return Err(anyhow!(
                "JS entry path contains unsupported component: {}",
                relative.display()
            ));
        };
        let part = part.to_str().ok_or_else(|| {
            anyhow!(
                "JS entry path contains non-UTF-8 component: {}",
                relative.display()
            )
        })?;

        if !first {
            guest.push('/');
        }
        guest.push_str(part);
        first = false;
    }

    if first {
        return Err(anyhow!("JS entry path cannot be the module root"));
    }

    Ok(guest)
}
