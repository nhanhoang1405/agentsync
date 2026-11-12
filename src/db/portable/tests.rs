use super::*;

fn contract(url: &str) {
    let mut db = PortableDatabase::connect(url, None).unwrap();
    db.migrate().unwrap();
    db.migrate().unwrap();
    let author = db.author_identity().unwrap();
    db.register_user(&author).unwrap();
    let context = SyncContext {
        scope: Scope::Global,
        project_root: std::path::PathBuf::new(),
        project_key: String::new(),
    };
    let modified = UNIX_EPOCH + Duration::from_micros(1_234_567_890);
    let resource = LocalResource {
        kind: ResourceKind::Skills,
        path: "skills/test/SKILL.md".into(),
        content: b"binary\x00\xff".to_vec(),
        source: Default::default(),
        modified_at: modified,
    };
    let history = LocalResource {
        kind: ResourceKind::Histories,
        path: "sessions/test.jsonl".into(),
        content: b"private history".to_vec(),
        source: Default::default(),
        modified_at: modified,
    };
    assert_eq!(
        db.push_with_visibility(
            &author,
            AgentName::Codex,
            &context,
            &[
                (&resource, Visibility::Public),
                (&history, Visibility::Public)
            ]
        )
        .unwrap(),
        2
    );
    let pulled = db
        .pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Skills),
        )
        .unwrap();
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].content, resource.content);
    assert_eq!(pulled[0].modified_at, Some(modified));
    assert_eq!(pulled[0].sync_version, 1);
    db.push_with_visibility(
        &author,
        AgentName::Codex,
        &context,
        &[(&resource, Visibility::Public)],
    )
    .unwrap();
    assert_eq!(
        db.pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Skills)
        )
        .unwrap()[0]
            .sync_version,
        2
    );
    let public = db
        .pull(
            "other@example.com",
            &author,
            AgentName::Codex,
            &context,
            None,
        )
        .unwrap();
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].kind, ResourceKind::Skills);
    let histories = db
        .pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Histories),
        )
        .unwrap();
    assert_eq!(histories[0].visibility, Visibility::Private);
    assert_eq!(
        db.skills("other@example.com", AgentName::Codex)
            .unwrap()
            .len(),
        1
    );
    let listed = db
        .list(ListFilter {
            viewer_email: &author,
            agent: AgentName::Codex,
            scope: Some(Scope::Global),
            kind: Some(ResourceKind::Skills),
            project_key: None,
            visibility: Some(Visibility::Public),
            author: Some(&author),
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].size, resource.content.len() as i64);
    assert_eq!(listed[0].sync_version, 2);
    assert!(!listed[0].updated_at.is_empty());
    // Database enforcement is independent of the application's privacy default.
    let privacy = db.runtime.block_on(
        sqlx::query("UPDATE agentsync_resources SET visibility='public' WHERE kind='histories'")
            .execute(&mut db.connection),
    );
    assert!(privacy.is_err());
    assert!(db.ping().is_ok());
}

#[test]
fn sqlite_persists_binary_content_versions_timestamps_and_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    contract(&url);
    let mut db = PortableDatabase::connect(&url, None).unwrap();
    db.migrate().unwrap();
    assert_eq!(db.author_identity().unwrap(), "sqlite@local");
    assert_eq!(
        db.skills("sqlite@local", AgentName::Codex).unwrap().len(),
        1
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("archive.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn sqlite_rolls_back_a_batch_when_one_resource_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let mut db = PortableDatabase::connect(&url, None).unwrap();
    db.migrate().unwrap();
    db.register_user("sqlite@local").unwrap();
    let context = SyncContext {
        scope: Scope::Global,
        project_root: Default::default(),
        project_key: String::new(),
    };
    let good = LocalResource {
        kind: ResourceKind::Skills,
        path: "skills/test/valid".into(),
        content: vec![1],
        source: Default::default(),
        modified_at: UNIX_EPOCH,
    };
    let bad = LocalResource {
        path: "../escape".into(),
        ..good.clone()
    };
    assert!(
        db.push_with_visibility(
            "sqlite@local",
            AgentName::Codex,
            &context,
            &[(&good, Visibility::Private), (&bad, Visibility::Private)]
        )
        .is_err()
    );
    assert!(
        db.skills("sqlite@local", AgentName::Codex)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn sqlite_rejects_unknown_schema_versions_without_rewriting_data() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("future.db").display());
    let mut db = PortableDatabase::connect(&url, None).unwrap();
    db.migrate().unwrap();
    db.runtime
        .block_on(
            sqlx::query("UPDATE agentsync_schema SET version=999").execute(&mut db.connection),
        )
        .unwrap();
    assert!(
        db.migrate()
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
    let version: i64 = db
        .runtime
        .block_on(
            sqlx::query_scalar("SELECT version FROM agentsync_schema")
                .fetch_one(&mut db.connection),
        )
        .unwrap();
    assert_eq!(version, 999);
}
#[test]
#[ignore = "requires a disposable empty database in AGENTSYNC_TEST_MYSQL_URL"]
fn mysql_contract() {
    contract(
        &std::env::var("AGENTSYNC_TEST_MYSQL_URL")
            .expect("set AGENTSYNC_TEST_MYSQL_URL to an empty test database"),
    );
}
