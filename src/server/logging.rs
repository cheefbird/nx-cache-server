use axum::{
    extract::Request,
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tracing::{Instrument, Level, Span};

#[derive(Clone, Copy, Debug)]
pub enum Access {
    ReadWrite,
    ReadOnly,
}

impl Access {
    fn as_str(self) -> &'static str {
        match self {
            Access::ReadWrite => "read_write",
            Access::ReadOnly => "read_only",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit,
    Miss,
    Stored,
    Exists,
    Forbidden,
    Unauthorized,
    Invalid,
    Error,
    Health,
    Aborted,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Hit => "hit",
            Outcome::Miss => "miss",
            Outcome::Stored => "stored",
            Outcome::Exists => "exists",
            Outcome::Forbidden => "forbidden",
            Outcome::Unauthorized => "unauthorized",
            Outcome::Invalid => "invalid",
            Outcome::Error => "error",
            Outcome::Health => "health",
            Outcome::Aborted => "aborted",
        }
    }

    /// Fallback when nothing recorded an outcome, e.g. axum rejections.
    fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::UNAUTHORIZED => Outcome::Unauthorized,
            StatusCode::FORBIDDEN => Outcome::Forbidden,
            StatusCode::CONFLICT => Outcome::Exists,
            s if s.is_server_error() => Outcome::Error,
            _ => Outcome::Invalid,
        }
    }
}

#[derive(Default)]
struct RecordFields {
    hash: Option<String>,
    access: Option<Access>,
    outcome: Option<Outcome>,
    bytes: u64,
}

/// Filled in by inner layers, read by `request_log`.
#[derive(Clone, Default)]
pub struct RequestRecord(Arc<Mutex<RecordFields>>);

impl RequestRecord {
    fn fields(&self) -> MutexGuard<'_, RecordFields> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Validated hashes only; never log raw client input.
    pub fn set_hash(&self, hash: &str) {
        self.fields().hash = Some(hash.to_owned());
        Span::current().record("hash", hash);
    }

    pub fn set_access(&self, access: Access) {
        self.fields().access = Some(access);
    }

    pub fn set_outcome(&self, outcome: Outcome) {
        self.fields().outcome = Some(outcome);
    }

    pub fn set_bytes(&self, bytes: u64) {
        self.fields().bytes = bytes;
    }
}

/// One line per request. `duration_ms` excludes streaming the response body.
pub async fn request_log(mut request: Request, next: Next) -> Response {
    let record = RequestRecord::default();
    request.extensions_mut().insert(record.clone());

    let mut line = RequestLine {
        record,
        method: request.method().clone(),
        started: Instant::now(),
        logged: false,
    };
    let span = tracing::info_span!("request", method = %line.method, hash = tracing::field::Empty);

    let response = next.run(request).instrument(span).await;

    line.log(Some(response.status()));
    response
}

/// Logs on drop if no response was produced, i.e. the client went away.
struct RequestLine {
    record: RequestRecord,
    method: Method,
    started: Instant,
    logged: bool,
}

impl RequestLine {
    fn log(&mut self, status: Option<StatusCode>) {
        self.logged = true;

        let duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let fields = self.record.fields();
        let outcome = match status {
            None => Outcome::Aborted,
            Some(status) => fields
                .outcome
                .unwrap_or_else(|| Outcome::from_status(status)),
        };
        let access = fields.access.map_or("none", Access::as_str);
        let hash = fields.hash.as_deref();
        let bytes = fields.bytes;
        let method = &self.method;
        let status_code = status.map(|s| s.as_u16());

        // `event!` needs a const level.
        macro_rules! request_event {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    method = %method,
                    hash,
                    status = status_code,
                    outcome = outcome.as_str(),
                    access,
                    bytes,
                    duration_ms,
                    "request"
                )
            };
        }

        if outcome == Outcome::Health {
            request_event!(Level::DEBUG);
        } else if status.is_some_and(|s| s.is_server_error()) {
            request_event!(Level::ERROR);
        } else {
            request_event!(Level::INFO);
        }
    }
}

impl Drop for RequestLine {
    fn drop(&mut self) {
        if !self.logged {
            self.log(None);
        }
    }
}
