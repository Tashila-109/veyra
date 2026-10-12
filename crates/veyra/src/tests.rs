pub(crate) struct Scratch(pub std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove owned scratch: {error}"),
        }
    }
}

#[test]
fn scratch_removes_setup_failures_and_absent_folders() {
    let root = std::path::Path::new("/tmp").join(format!(
        "veyra-scratch-{}",
        veyra_engine::domain::ThreadId::new()
    ));
    assert!(
        std::panic::catch_unwind(|| {
            let _scratch = Scratch(root.clone());
            std::fs::create_dir(&root).unwrap();
            panic!("setup failed");
        })
        .is_err()
    );
    assert!(!root.exists());
    drop(Scratch(root.clone()));
    assert!(!root.exists());
}
