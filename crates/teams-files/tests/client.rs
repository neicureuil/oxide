use http::{HeaderMap, HeaderValue};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use teams_auth::{AuthError, Authenticator, CallingContext, Service};
use teams_files::{
    CHUNK_SIZE, Destination, DriveEntry, Error, FilesClient, SpoError, share_id,
};
use url::Url;
use wiremock::matchers::{body_json, body_string, header, headers, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

struct Auth {
    base: Url,
    refreshes: AtomicUsize,
}
impl Authenticator for Auth {
    fn base_url(&self, _service: Service) -> Result<Url, AuthError> {
        Ok(self.base.clone())
    }
    async fn authorize(&self, service: Service, h: &mut HeaderMap) -> Result<(), AuthError> {
        h.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {service:?}")).unwrap(),
        );
        Ok(())
    }
    async fn authorize_resource(&self, url: &Url, h: &mut HeaderMap) -> Result<(), AuthError> {
        assert_eq!(url.origin(), self.base.origin(), "token sent to another host");
        let token = if self.refreshes.load(Ordering::SeqCst) == 0 {
            "Bearer site"
        } else {
            "Bearer renewed"
        };
        h.insert("authorization", HeaderValue::from_static(token));
        Ok(())
    }
    async fn on_resource_unauthorized(&self, _url: &Url) -> bool {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn calling_context(&self) -> Option<CallingContext> {
        Some(CallingContext {
            tenant_id: Some("home-tenant".into()),
            ..Default::default()
        })
    }
}
fn client(server: &MockServer) -> FilesClient<Auth> {
    FilesClient::new(Arc::new(Auth {
        base: Url::parse(&format!("{}/", server.uri())).unwrap(),
        refreshes: AtomicUsize::new(0),
    }))
}
fn site(server: &MockServer) -> Url {
    Url::parse(&format!("{}/personal/a/", server.uri())).unwrap()
}
fn temp(name: &str, size: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("teams-files-{}", uuid()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, vec![7u8; size]).unwrap();
    file
}
fn uuid() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}
const GUID: &str = "0fb7e6b8-1b9f-4c3e-9b39-6a0bc1f7c111";

#[tokio::test]
async fn availability_is_a_bodyless_post_to_the_middle_tier() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/beta/me/files/availability"))
        .and(header("authorization", "Bearer MiddleTier"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "personalSiteUrl": format!("{}/personal/a", server.uri()),
            "personalRootFolderUrl": format!("{}/personal/a/Documents", server.uri()),
        })))
        .expect(1)
        .mount(&server)
        .await;
    let site = client(&server).personal_site().await.unwrap();
    assert_eq!(site.chat_folder("Microsoft Teams Chat Files"), "/personal/a/Documents/Microsoft Teams Chat Files");
}

#[tokio::test]
async fn small_chat_files_use_one_quick_upload_put() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/personal/a/_api/v2.0/drive/root:/Microsoft%20Teams%20Chat%20Files/r%C3%A9sum%C3%A9.txt:/content"))
        .and(query_param("@name.conflictBehavior", "rename"))
        .and(query_param("$select", "*,sharepointIds,webDavUrl"))
        .and(header("scenario", "ShareUploadFile"))
        .and(header("scenariotype", "AUO"))
        .and(header("prefer", "manualRedirect"))
        .and(header("application", "Teams_Web"))
        .and(header("content-type", "application/octet-stream"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "01ITEM", "name": "résumé.txt", "size": 5,
            "webDavUrl": format!("{}/personal/a/Documents/Microsoft Teams Chat Files/résumé.txt", server.uri()),
            "parentReference": {"driveId": "b!drive"},
            "sharepointIds": {"listItemUniqueId": GUID, "siteUrl": format!("{}/personal/a", server.uri())}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let file = temp("local.txt", 5);
    let calls = Mutex::new(vec![]);
    let uploaded = client(&server)
        .upload(
            &Destination {
                site: site(&server),
                folder: "/personal/a/Documents/Microsoft Teams Chat Files".into(),
                library_relative: Some("Microsoft Teams Chat Files".into()),
            },
            &file,
            "résumé.txt",
            &|done, total| calls.lock().unwrap().push((done, total)),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.file_id, GUID);
    assert_eq!(uploaded.item_id.as_deref(), Some("01ITEM"));
    assert_eq!(uploaded.drive_id.as_deref(), Some("b!drive"));
    assert_eq!(uploaded.site_url, site(&server).as_str());
    assert_eq!(calls.lock().unwrap().last(), Some(&(5, 5)));
}

#[tokio::test]
async fn chunked_upload_creates_the_folder_renames_and_sends_ten_megabyte_chunks() {
    let server = MockServer::start().await;
    let folder = "/sites/t/Shared Documents/General";
    // First stub: folder missing, then a name conflict, then success under "r 2.bin".
    Mock::given(method("POST"))
        .and(path("/sites/t/_api/web/GetFolderByServerRelativeUrl(@v)/Files/addStub(@fn)"))
        .and(query_param("@fn", "'r.bin'"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"odata.error": {"code": "-2147024893, System.IO.DirectoryNotFoundException"}})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sites/t/_api/web/folders/AddUsingPath(DecodedUrl=@a1,overwrite=@a2)"))
        .and(query_param("@a1", format!("'{folder}'")))
        .and(query_param("@a2", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sites/t/_api/web/GetFolderByServerRelativeUrl(@v)/Files/addStub(@fn)"))
        .and(query_param("@fn", "'r.bin'"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"odata.error": {"code": "-2130575257, Microsoft.SharePoint.SPException"}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sites/t/_api/web/GetFolderByServerRelativeUrl(@v)/Files/addStub(@fn)"))
        .and(query_param("@v", format!("'{folder}'")))
        .and(query_param("@fn", "'r 2.bin'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "UniqueId": GUID, "ServerRelativeUrl": format!("{folder}/r 2.bin")
        })))
        .expect(1)
        .mount(&server)
        .await;
    let size = CHUNK_SIZE as usize + 2048;
    let lengths = Arc::new(Mutex::new(vec![]));
    let seen = lengths.clone();
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"^/sites/t/_api/web/GetFileById\('0fb7e6b8-1b9f-4c3e-9b39-6a0bc1f7c111'\)/(Start|Continue)Upload\(uploadId=guid'[0-9a-f-]{36}'(,fileOffset=\d+)?\)$"))
        .and(query_param("@file", "'r 2.bin'"))
        .respond_with(move |request: &Request| {
            let mut seen = seen.lock().unwrap();
            let offset: usize = seen.iter().sum();
            seen.push(request.body.len());
            ResponseTemplate::new(200).set_body_json(json!({"value": offset + request.body.len()}))
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(format!(
            r"^/sites/t/_api/web/GetFileById\('{GUID}'\)/FinishUpload\(uploadId=guid'[0-9a-f-]{{36}}',fileOffset={size}\)$"
        )))
        .and(body_string(""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let file = temp("r.bin", size);
    let uploaded = client(&server)
        .upload(
            &Destination {
                site: Url::parse(&format!("{}/sites/t", server.uri())).unwrap(),
                folder: folder.into(),
                library_relative: None,
            },
            &file,
            "r.bin",
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    assert_eq!(*lengths.lock().unwrap(), [CHUNK_SIZE as usize, 2048]);
    assert_eq!(uploaded.name, "r 2.bin");
    assert_eq!(
        uploaded.object_url,
        format!("{}/sites/t/Shared%20Documents/General/r%202.bin", server.uri())
    );
}

#[tokio::test]
async fn a_failed_chunk_cancels_the_session_and_deletes_the_stub() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/personal/a/_api/web/GetFolderByServerRelativeUrl(@v)/Files/addStub(@fn)"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"UniqueId": GUID})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"/StartUpload\("))
        .respond_with(ResponseTemplate::new(507).set_body_json(json!({"odata.error": {"code": "-2147023080, quota"}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"/CancelUpload\("))
        .and(header("scenario", "ShareAbort"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/personal/a/_api/web/GetFileById('{GUID}')")))
        .and(header("scenariotype", "PO"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let file = temp("big.bin", 10);
    let error = client(&server)
        .upload(
            &Destination {
                site: site(&server),
                folder: "/personal/a/Documents/x".into(),
                library_relative: None,
            },
            &file,
            "big.bin",
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .await
        .unwrap_err();
    assert_eq!(error.spo(), Some(&SpoError::QuotaReached));
}

#[tokio::test]
async fn invite_grants_members_by_object_id_and_reads_the_link() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/personal/a/_api/v2.0/sites/root/items/{GUID}/driveItem/invite")))
        .and(headers("prefer", vec!["ReturnSpecificErrorCode", "getDefaultLink"]))
        .and(header("scenario", "TeamsFiles"))
        .and(body_json(json!({"recipients": [{"objectId": "oid-b"}]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"id": "perm", "link": {"webUrl": "https://c-my.sharepoint.com/:t:/g/personal/a/E1", "scope": "users"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let shared = client(&server)
        .invite(&site(&server), Some(GUID), "", &["oid-b".into()])
        .await
        .unwrap();
    assert_eq!(shared.share_id.as_deref(), Some("perm"));
    assert_eq!(shared.share_url.as_deref(), Some("https://c-my.sharepoint.com/:t:/g/personal/a/E1"));
}

#[tokio::test]
async fn redeem_retries_with_a_renewed_token_then_falls_back_to_the_item() {
    let server = MockServer::start().await;
    let link = format!("{}/:w:/g/personal/a/E1", server.uri());
    Mock::given(method("GET"))
        .and(path(format!("/_api/v2.0/shares/{}/driveItem/", share_id(&link))))
        .and(headers("prefer", vec!["redeemSharingLink", "getShortLivedDownloadUrl"]))
        .and(header("authorization", "Bearer site"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/_api/v2.0/shares/{}/driveItem/", share_id(&link))))
        .and(header("authorization", "Bearer renewed"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/personal/a/_api/v2.0/sites/root/items/{GUID}/driveItem/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "a.docx", "size": 4,
            "@microsoft.graph.downloadUrl": format!("{}/download/a?tempauth=x", server.uri())
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/download/a"))
        .respond_with(|request: &Request| {
            // A tempauth URL needs no credentials: none must be sent.
            assert!(request.headers.get("authorization").is_none());
            ResponseTemplate::new(200).set_body_bytes(b"data".to_vec())
        })
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    let info = client.redeem(&link, Some((&site(&server), GUID))).await.unwrap();
    let dest = temp("unused", 0).with_file_name("a.docx");
    let size = client
        .download(info.download_url.as_deref().unwrap(), &dest, &|_, _| {}, &AtomicBool::new(false))
        .await
        .unwrap();
    assert_eq!(size, 4);
    assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    assert!(!dest.with_file_name("a.docx.part").exists());
}

#[tokio::test]
async fn a_token_refused_by_another_organizations_site_is_told_apart() {
    let server = MockServer::start().await;
    let link = format!("{}/:w:/g/personal/a/E1", server.uri());
    for (realm, expected) in [
        ("OTHER-TENANT", SpoError::ForeignTenant),
        ("HOME-TENANT", SpoError::InvalidAudience),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("www-authenticate", format!(r#"Bearer realm="{realm}",client_id="x""#))
                    .set_body_json(json!({"error": {"code": "invalidAudienceUri", "message": "Invalid audience Uri"}})),
            )
            .mount(&server)
            .await;
        let error = client(&server).redeem(&link, None).await.unwrap_err();
        assert_eq!(error.spo(), Some(&expected), "{realm}");
    }
}

#[tokio::test]
async fn a_site_tells_its_organization_to_an_empty_bearer() {
    let server = MockServer::start().await;
    for (realm, foreign) in [("OTHER-TENANT", true), ("HOME-TENANT", false)] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/_vti_bin/client.svc"))
            .and(header("authorization", "Bearer"))
            .respond_with(ResponseTemplate::new(401).insert_header(
                "www-authenticate",
                format!(r#"Bearer realm="{realm}",client_id="x""#),
            ))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            client(&server)
                .is_foreign_site(&site(&server))
                .await
                .unwrap(),
            foreign,
            "{realm}"
        );
    }
}

#[tokio::test]
async fn substrate_lists_use_the_documented_filters_and_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/allfiles/api/me/interactions('SharedInChatFast')/items"))
        .and(query_param("$filter", "callerSharingInfo/chatIds/any(n: n eq '19:abc@thread.v2')"))
        .and(header("authorization", "Bearer Substrate"))
        .and(header("scenariotag", "TeamsSharedInChat_AllFiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"name": "a.pdf", "file": {"extension": "pdf"}}]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/beta/me/WorkingSetFiles"))
        .and(query_param("top", "50"))
        .respond_with(|request: &Request| {
            let filter = request
                .url
                .query_pairs()
                .find(|(k, _)| k == "$filter")
                .unwrap()
                .1
                .into_owned();
            assert!(filter.contains("StorageProviderContext eq 11"));
            assert!(filter.ends_with("TeamsMessageThreadId eq '19:abc@thread.v2')"));
            assert_eq!(
                request.headers.get("prefer").unwrap(),
                "exchange.behavior=\"ApplicationData,SubstrateFiles\""
            );
            ResponseTemplate::new(200).set_body_json(json!({"value": [{"Id": "1", "ItemProperties": {"WebLink": {"WebUrl": "https://example.com/"}}}]}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    assert_eq!(client.shared_in_chat("19:abc@thread.v2", true).await.unwrap()[0].name, "a.pdf");
    assert_eq!(client.shared_links("19:abc@thread.v2").await.unwrap()[0].title, "example.com");
}

#[tokio::test]
async fn folders_list_create_rename_and_recycle() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sites/t/_api/web/GetFolderByServerRelativePath(decodedUrl=@a1)"))
        .and(query_param("@a1", "'/sites/t/Shared Documents/General'"))
        .and(query_param("$expand", "Folders,Files,Files/ModifiedBy"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Folders": [{"Name": "Specs", "ServerRelativeUrl": "/sites/t/Shared Documents/General/Specs", "UniqueId": GUID}],
            "Files": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sites/t/_api/web/GetFolderByServerRelativePath(decodedUrl=@a1)/Folders"))
        .and(query_param("@a1", "'/sites/t/Shared Documents/General'"))
        .and(query_param("$select", "UniqueId,ListItemAllFields/OData__ColorHex"))
        .and(query_param("$expand", "ListItemAllFields"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"UniqueId": GUID, "ListItemAllFields": {"OData__ColorHex": "3"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sites/t/_api/web/folders/AddUsingPath(DecodedUrl=@a1,overwrite=@a2)"))
        .and(query_param("@a1", "'/sites/t/Shared Documents/General/Nouveau dossier'"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/sites/t/_api/web/GetFolderById('{GUID}')/ListItemAllFields/validateUpdateListItem")))
        .and(body_json(json!({"formValues": [{"FieldName": "FileLeafRef", "FieldValue": "Plans"}], "bNewDocumentUpdate": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"FieldName": "FileLeafRef", "HasException": false}]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/sites/t/_api/web/GetFolderById('{GUID}')/ListItemAllFields/validateUpdateListItem")))
        .and(body_json(json!({"formValues": [{"FieldName": "_ColorHex", "FieldValue": "12"}], "bNewDocumentUpdate": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"FieldName": "_ColorHex", "HasException": false}]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/sites/t/_api/web/GetFolderById('{GUID}')/recycle")))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    let site = Url::parse(&format!("{}/sites/t/", server.uri())).unwrap();
    let list = client.list_folder(&site, "/sites/t/Shared Documents/General").await.unwrap();
    let folder: &DriveEntry = &list.entries[0];
    assert!(folder.folder);
    assert_eq!(folder.color, 3);
    client.create_folder(&site, "/sites/t/Shared Documents/General/", "Nouveau dossier").await.unwrap();
    client.rename(&site, folder, "Plans").await.unwrap();
    client.set_color(&site, folder, 12).await.unwrap();
    assert!(matches!(client.set_color(&site, folder, 16).await, Err(Error::Invalid(_))));
    client.delete(&site, folder).await.unwrap();
    assert!(matches!(
        client.rename(&site, folder, "a/b").await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn search_runs_on_the_default_library_and_keeps_site_paths() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sites/t/_api/v2.0/drive/root/search(q='l''%C3%A9t%C3%A9')"))
        .and(query_param("$top", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
            {"id": "1", "name": "l'été.docx", "size": 3, "file": {},
             "webUrl": format!("{}/sites/t/Shared%20Documents/General/l'%C3%A9t%C3%A9.docx", server.uri())},
            {"id": "2", "name": "Ailleurs", "folder": {"childCount": 0}, "webUrl": "https://other.example/x"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;
    let site = Url::parse(&format!("{}/sites/t/", server.uri())).unwrap();
    let client = client(&server);
    let found = client.search(&site, " l'été ").await.unwrap().entries;
    assert_eq!(
        found[0].server_relative_url.as_deref(),
        Some("/sites/t/Shared Documents/General/l'été.docx")
    );
    assert_eq!(found[1].server_relative_url, None);
    assert!(client.search(&site, "  ").await.unwrap().entries.is_empty());
}
