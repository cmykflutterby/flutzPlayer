use flutz_core::FmidProject;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenProjectState {
    pub project: FmidProject,
    pub dirty: bool,
}
