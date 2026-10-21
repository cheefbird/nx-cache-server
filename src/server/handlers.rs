use crate::domain::storage::{StorageError, StorageProvider};
use crate::server::{
    error::ServerError,
    logging::{Outcome, RequestRecord},
    validation, AppState,
};
use axum::{
    body::Body,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
};

pub async fn store_artifact<T: StorageProvider>(
    Path(hash): Path<String>,
    State(state): State<AppState<T>>,
    Extension(record): Extension<RequestRecord>,
    body: Body,
) -> Result<impl IntoResponse, ServerError> {
    validation::validate_hash(&hash)?;

    if state.storage.exists(&hash).await? {
        // Same reason as the 403 in auth_middleware: let the client finish
        // uploading, or it never sees this 409. Keys are content-addressed, so
        // the copy being discarded is byte-identical to the stored one.
        record.set_outcome(Outcome::Exists);
        crate::server::drain_body(body).await;
        return Ok((StatusCode::CONFLICT, "Cannot override an existing record"));
    }

    // For now, let's use a simpler approach - collect the body into bytes
    // TODO: Implement true streaming later for better memory efficiency
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| ServerError::BadRequest)?;

    let length = bytes.len() as u64;
    let cursor = std::io::Cursor::new(bytes);
    let reader_stream = tokio_util::io::ReaderStream::new(cursor);

    match state.storage.store(&hash, reader_stream).await {
        Ok(()) => {
            record.set_outcome(Outcome::Stored);
            record.set_bytes(length);
        }
        Err(e) => {
            if matches!(e, StorageError::AlreadyExists) {
                record.set_outcome(Outcome::Exists);
            }
            return Err(e.into());
        }
    }

    Ok((StatusCode::OK, ""))
}

pub async fn retrieve_artifact<T: StorageProvider>(
    Path(hash): Path<String>,
    State(state): State<AppState<T>>,
    Extension(record): Extension<RequestRecord>,
) -> Result<impl IntoResponse, ServerError> {
    validation::validate_hash(&hash)?;

    let reader = match state.storage.retrieve(&hash).await {
        Ok(reader) => {
            record.set_outcome(Outcome::Hit);
            reader
        }
        Err(e) => {
            if matches!(e, StorageError::NotFound) {
                record.set_outcome(Outcome::Miss);
            }
            return Err(e.into());
        }
    };
    let stream = tokio_util::io::ReaderStream::new(reader);
    let body = Body::from_stream(stream);

    Ok((
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        body,
    ))
}

pub async fn health_check(Extension(record): Extension<RequestRecord>) -> impl IntoResponse {
    record.set_outcome(Outcome::Health);
    (StatusCode::OK, "OK")
}
