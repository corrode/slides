use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        let error = error.into();
        tracing::error!(error = ?error, "request failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Something went wrong while processing the request.".into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = html_escape::encode_text(&self.message);
        (
            self.status,
            Html(format!(
                "<div class=\"notice error\" role=\"alert\">{message}</div>"
            )),
        )
            .into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
