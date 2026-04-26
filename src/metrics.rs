use std::time::Duration;

use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram, UpDownCounter},
};

/// OpenTelemetry instruments used by the HTTP handler.
#[derive(Debug, Clone)]
pub(crate) struct HttpMetrics {
    requests: Counter<u64>,
    duration: Histogram<f64>,
    response_body_size: Histogram<u64>,
    in_flight: UpDownCounter<i64>,
}

impl HttpMetrics {
    /// Creates HTTP metric instruments from the global OpenTelemetry meter.
    ///
    /// # Returns
    /// A handle containing reusable metric instruments.
    #[must_use]
    pub(crate) fn new() -> Self {
        let meter = global::meter("tiny-httpd");
        Self {
            requests: meter
                .u64_counter("http.server.request.count")
                .with_description("Completed HTTP requests")
                .build(),
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("HTTP request duration")
                .build(),
            response_body_size: meter
                .u64_histogram("http.server.response.body.size")
                .with_unit("By")
                .with_description("HTTP response body size")
                .build(),
            in_flight: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_description("HTTP requests currently in flight")
                .build(),
        }
    }

    /// Records request start by incrementing the in-flight request gauge.
    ///
    /// # Returns
    /// A guard that decrements the gauge when dropped.
    pub(crate) fn request_started(&self) -> InFlightRequestGuard<'_> {
        self.in_flight.add(1, &[]);
        InFlightRequestGuard {
            metrics: self,
            finished: false,
        }
    }

    /// Records request completion metrics.
    ///
    /// # Arguments
    /// * `method` - HTTP method name.
    /// * `status` - Numeric HTTP response status.
    /// * `elapsed` - Time spent handling the request.
    /// * `response_bytes` - Response body bytes reported by the HTTP `Content-Length` header when present.
    pub(crate) fn request_finished(
        &self,
        method: &str,
        status: u16,
        elapsed: Duration,
        response_bytes: u64,
    ) {
        let attributes = [
            KeyValue::new("http.request.method", method.to_string()),
            KeyValue::new("http.response.status_class", status_class(status)),
        ];
        self.requests.add(1, &attributes);
        self.duration.record(elapsed.as_secs_f64(), &attributes);
        self.response_body_size.record(response_bytes, &attributes);
    }
}

/// RAII guard tracking one in-flight request.
#[derive(Debug)]
#[must_use = "dropping the guard immediately decrements the in-flight counter"]
pub(crate) struct InFlightRequestGuard<'a> {
    metrics: &'a HttpMetrics,
    finished: bool,
}

impl InFlightRequestGuard<'_> {
    /// Marks the request as finished so the in-flight gauge is decremented exactly once.
    pub(crate) fn finish(&mut self) {
        if !self.finished {
            self.metrics.in_flight.add(-1, &[]);
            self.finished = true;
        }
    }
}

impl Drop for InFlightRequestGuard<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

impl Default for HttpMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps a numeric HTTP status code to its `Nxx` status class.
///
/// # Arguments
/// * `status` - Numeric HTTP response status.
///
/// # Returns
/// The corresponding HTTP status class label.
#[must_use]
pub(crate) fn status_class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => "other",
    }
}
