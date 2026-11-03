use http::{HeaderName, HeaderValue};
use std::sync::Arc;
use teams_auth::{Service, StaticAuthenticator};
use teams_chat::ChatClient;
use url::Url;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> ChatClient<StaticAuthenticator> {
    ChatClient::new(Arc::new(
        StaticAuthenticator::new()
            .with_service(
                Service::Ams,
                Url::parse(&format!("{}/media/", server.uri())).unwrap(),
            )
            .with_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer private"),
            ),
    ))
}
const SOURCE: &str = "https://eu-api.asm.skype.com/v1/objects/image/views/imgo?foo=bar";
#[tokio::test]
async fn ams_is_rewritten_and_headers_are_removed_on_non_ams_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/media/v1/objects/image/views/imgo"))
        .and(header("authorization", "Bearer private"))
        .and(query_param("v", "1"))
        .and(query_param("foo", "bar"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/public/image.png"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/public/image.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"image bytes"))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client(&server).download_image(SOURCE).await.unwrap(),
        b"image bytes"
    );
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[1].headers.contains_key("authorization"));
    assert!(!requests[1].headers.contains_key("x-skypetoken"));
}
#[tokio::test]
async fn card_images_through_the_url_preview_proxy_are_authenticated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/urlp/v1/url/content"))
        .and(header("authorization", "Bearer private"))
        .and(query_param("url", "https://teams.github.com/pr-icon.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"icon"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/media/v1/objects/image/views/imgo"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            "/urlp/v1/url/content?url=https%3a%2f%2fteams.github.com%2fpr-icon.png",
        ))
        .mount(&server)
        .await;
    assert_eq!(client(&server).download_image(SOURCE).await.unwrap(), b"icon");
}
#[tokio::test]
async fn insecure_external_redirects_and_oversized_images_are_rejected() {
    let server = MockServer::start().await;
    let external = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&external)
        .await;
    Mock::given(path("/media/v1/objects/image/views/imgo"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/steal", external.uri())),
        )
        .mount(&server)
        .await;
    assert!(client(&server).download_image(SOURCE).await.is_err());
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 8 * 1024 * 1024 + 1]))
        .mount(&server)
        .await;
    assert!(
        client(&server)
            .download_image(SOURCE)
            .await
            .unwrap_err()
            .to_string()
            .contains("volumineuse")
    );
}

#[tokio::test]
async fn profile_pictures_use_authenticated_regional_service_and_bound_downloads() {
    let server = MockServer::start().await;
    let auth = StaticAuthenticator::new()
        .with_service(
            Service::ProfileImages,
            Url::parse(&format!("{}/images/", server.uri())).unwrap(),
        )
        .with_header(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer private"),
        );
    let client = ChatClient::new(Arc::new(auth));
    Mock::given(method("GET"))
        .and(path(
            "/images/beta/users/8%3Aorgid%3Aalice/profilepicturev2",
        ))
        .and(header("authorization", "Bearer private"))
        .and(query_param("size", "HR96x96"))
        .and(query_param("displayname", "Alice Martin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3]))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client
            .profile_picture("8:orgid:alice", "Alice Martin")
            .await
            .unwrap(),
        vec![1, 2, 3]
    );
    Mock::given(method("GET"))
        .and(path("/images/beta/users/large/profilepicturev2"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 2 * 1024 * 1024 + 1]))
        .expect(1)
        .mount(&server)
        .await;
    assert!(client.profile_picture("large", "").await.is_err());
    Mock::given(method("GET"))
        .and(path("/images/beta/users/missing/profilepicturev2"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    assert!(client.profile_picture("missing", "").await.is_err());
}
#[tokio::test]
async fn legacy_url_preview_thumbnails_are_fetched_from_urlp_v2() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/url/content"))
        .and(header("authorization", "Bearer private"))
        .and(query_param("url", "https://www.laprovence.com/a.jpg"))
        .and(query_param("v", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"thumbnail"))
        .expect(1)
        .mount(&server)
        .await;
    let client = ChatClient::new(Arc::new(
        StaticAuthenticator::new()
            .with_service(Service::UrlPreview, Url::parse(&format!("{}/urlp/", server.uri())).unwrap())
            .with_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer private"),
            ),
    ));
    let legacy = "https://urlp.asm.skype.com/v1/url/content?url=https%3A%2F%2Fwww.laprovence.com%2Fa.jpg";
    assert_eq!(client.download_image(legacy).await.unwrap(), b"thumbnail");
}
#[tokio::test]
async fn preview_images_carry_the_ams_cookie_as_the_web_client_s_img_does() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/media/v1/aadtokenauth"))
        .and(header("authorization", "Bearer private"))
        .respond_with(
            ResponseTemplate::new(204)
                .append_header("set-cookie", "authtoken=cookie-value; Path=/; Secure; HttpOnly")
                .append_header("set-cookie", "other=x; Domain=elsewhere.test"),
        )
        .expect(1)
        .mount(&server)
        .await;
    // The proxy refuses a bearer, as the real one did, and takes the cookie alone.
    Mock::given(method("GET"))
        .and(path("/v1/url/content"))
        .and(header("cookie", "authtoken=cookie-value"))
        .and(wiremock::matchers::header_exists("authorization"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/url/content"))
        .and(header("cookie", "authtoken=cookie-value"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"thumbnail"))
        .mount(&server)
        .await;
    let base = Url::parse(&format!("{}/media/", server.uri())).unwrap();
    let client = ChatClient::new(Arc::new(
        StaticAuthenticator::new()
            .with_service(Service::Ams, base.clone())
            .with_service(Service::UrlPreview, base)
            .with_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer private"),
            ),
    ));
    let legacy = "https://urlp.asm.skype.com/v1/url/content?url=https%3A%2F%2Fx.com%2Fa.jpg";
    assert_eq!(client.download_image(legacy).await.unwrap(), b"thumbnail");
}
#[tokio::test]
async fn a_pasted_image_is_created_for_the_conversation_then_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/media/v1/objects/"))
        .and(header("authorization", "Bearer private"))
        .and(header("x-ms-client-version", teams_auth::CLIENT_VERSION))
        .and(wiremock::matchers::body_partial_json(serde_json::json!({
            "type": "pish/image",
            "permissions": {"19:chat@thread.v2": ["read"]},
            "filename": "image.png",
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "0-weu-d1-abc"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/media/v1/objects/0-weu-d1-abc/content/imgpsh"))
        .and(header("authorization", "Bearer private"))
        .and(wiremock::matchers::body_bytes(b"png bytes".to_vec()))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    let url = client(&server)
        .upload_image("19:chat@thread.v2", "image.png", b"png bytes".to_vec())
        .await
        .unwrap();
    assert_eq!(
        url,
        format!("{}/media/v1/objects/0-weu-d1-abc/views/imgo", server.uri())
    );
}
