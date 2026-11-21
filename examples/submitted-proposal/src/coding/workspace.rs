//! File operations and engine checkpoints for one shipping workspace.
use super::*;

pub(super) trait ShippingWorkspace {
    type Stage;
    fn path(&self) -> PathBuf;
    fn snapshot(&self) -> Result<Snapshot>;
    fn stage(&self, content: &str) -> Result<Self::Stage>;
    fn replace(
        &self,
        stage: &mut Self::Stage,
        before: &Snapshot,
        control: &RunControl,
    ) -> Result<()>;
    fn sync_workspace(&self) -> Result<()>;

    fn abandon(&self, _stage: Self::Stage, primary: String) -> String {
        primary
    }
    fn hit(&self, _point: &'static str) -> Result<()> {
        Ok(())
    }
    #[cfg(test)]
    fn inject_edit(&self, _point: &str, _bytes: &str) -> Result<()> {
        Ok(())
    }

    fn confirm_dispatch(
        &self,
        _request: &ProtectedEffectRequest,
        _events: &[RunEvent],
        _usage: &ResourceUsage,
        _reserved: &EffectUsage,
    ) -> std::result::Result<(), RuntimeError> {
        Ok(())
    }
    fn confirm_effect(
        &self,
        _result: &ProtectedEffectResult,
        _usage: &ResourceUsage,
        _record: &Value,
    ) -> std::result::Result<(), RuntimeError> {
        Ok(())
    }
    fn verifier_snapshot(&self, _snapshot: &Snapshot) -> Result<()> {
        Ok(())
    }
    fn confirm_verification(
        &self,
        _event: &RunEvent,
        _usage: &ResourceUsage,
        _report: &Value,
    ) -> std::result::Result<(), RuntimeError> {
        Ok(())
    }
    fn claim_completion(&self) -> Result<()> {
        Ok(())
    }
    fn check_completion(&self, _subject: &Subject, _usage: &ResourceUsage) -> Result<()> {
        Ok(())
    }
}

pub(super) struct LegacyWorkspace<'a> {
    pub store: &'a CodingStore,
    pub state: &'a State,
}

impl ShippingWorkspace for LegacyWorkspace<'_> {
    type Stage = Option<tempfile::NamedTempFile>;

    fn path(&self) -> PathBuf {
        self.store.workspace()
    }
    fn hit(&self, point: &'static str) -> Result<()> {
        self.store.hit(point)
    }
    fn snapshot(&self) -> Result<Snapshot> {
        self.store.snapshot(self.state)
    }
    fn stage(&self, content: &str) -> Result<Self::Stage> {
        self.hit("target_create")?;
        let mut stage = tempfile::NamedTempFile::new_in(self.path()).map_err(|e| e.to_string())?;
        self.hit("target_write")?;
        stage
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        self.hit("target_mode")?;
        stage
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        self.hit("target_flush")?;
        stage.flush().map_err(|e| e.to_string())?;
        self.hit("target_sync")?;
        stage.as_file().sync_all().map_err(|e| e.to_string())?;
        Ok(Some(stage))
    }
    fn replace(
        &self,
        stage: &mut Self::Stage,
        _before: &Snapshot,
        _control: &RunControl,
    ) -> Result<()> {
        stage
            .take()
            .ok_or("shipping stage already consumed")?
            .persist(self.path().join("shipping.py"))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn sync_workspace(&self) -> Result<()> {
        File::open(self.path())
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
    #[cfg(test)]
    fn inject_edit(&self, point: &str, bytes: &str) -> Result<()> {
        if self.store.fault == Some(point) {
            fs::write(self.path().join("shipping.py"), bytes).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
