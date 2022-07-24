// Copyright 2021 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::env;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;

use once_cell::sync::Lazy;
use opentelemetry::global;
use opentelemetry::sdk::propagation::TraceContextPropagator;
use sentry_tracing::EventFilter;
use tracing::Event;
use tracing::Level;
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::RollingFileAppender;
use tracing_appender::rolling::Rotation;
use tracing_bunyan_formatter::BunyanFormattingLayer;
use tracing_bunyan_formatter::JsonStorageLayer;
use tracing_log::LogTracer;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::FormatEvent;
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::fmt::FormattedFields;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;

/// Init tracing for unittest.
/// Write logs to file `unittest`.
pub fn init_default_ut_tracing() {
    static START: Once = Once::new();

    START.call_once(|| {
        let mut g = GLOBAL_UT_LOG_GUARD.as_ref().lock().unwrap();
        *g = Some(init_global_tracing(
            "unittest",
            "_logs_unittest",
            "DEBUG",
            None,
        ));
    });
}

static GLOBAL_UT_LOG_GUARD: Lazy<Arc<Mutex<Option<Vec<WorkerGuard>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(None)));

/// Init logging and tracing.
///
/// A local tracing collection(maybe for testing) can be done with a local jaeger server.
/// To report tracing data and view it:
///   docker run -d -p6831:6831/udp -p6832:6832/udp -p16686:16686 jaegertracing/all-in-one:latest
///   DATABEND_JAEGER_AGENT_ENDPOINT=localhost:6831 RUST_LOG=trace cargo test
///   open http://localhost:16686/
///
/// To adjust batch sending delay, use `OTEL_BSP_SCHEDULE_DELAY`:
/// DATABEND_JAEGER_AGENT_ENDPOINT=localhost:6831 RUST_LOG=trace OTEL_BSP_SCHEDULE_DELAY=1 cargo test
///
// TODO(xp): use DATABEND_JAEGER_AGENT_ENDPOINT to assign jaeger server address.
pub fn init_global_tracing(
    app_name: &str,
    dir: &str,
    level: &str,
    disable_stdout: Option<bool>,
) -> Vec<WorkerGuard> {
    let mut guards = vec![];

    // Enable log compatible layer to convert log record to tracing span.
    LogTracer::init().expect("log tracer must be valid");

    // JSON layer:
    // Log files will be stored in log.dir, default is '.databend/logs'.
    let rolling_appender = RollingFileAppender::new(Rotation::HOURLY, dir, app_name);
    let (rolling_writer, rolling_writer_guard) = tracing_appender::non_blocking(rolling_appender);
    let file_logging_layer = BunyanFormattingLayer::new(app_name.to_string(), rolling_writer);
    guards.push(rolling_writer_guard);

    // Jaeger layer.
    let mut jaeger_layer = None;
    let jaeger_agent_endpoint =
        env::var("DATABEND_JAEGER_AGENT_ENDPOINT").unwrap_or_else(|_| "".to_string());
    if !jaeger_agent_endpoint.is_empty() {
        global::set_text_map_propagator(TraceContextPropagator::new());

        let tracer = opentelemetry_jaeger::new_pipeline()
            .with_service_name(app_name)
            .with_agent_endpoint(jaeger_agent_endpoint)
            .with_auto_split_batch(true)
            .install_batch(opentelemetry::runtime::Tokio)
            .expect("install");

        jaeger_layer = Some(tracing_opentelemetry::layer().with_tracer(tracer));
    }

    // Sentry Layer.
    let mut sentry_layer = None;
    let bend_sentry_env = env::var("DATABEND_SENTRY_DSN").unwrap_or_else(|_| "".to_string());
    if !bend_sentry_env.is_empty() {
        sentry_layer = Some(
            sentry_tracing::layer()
                .event_filter(|metadata| match metadata.level() {
                    &Level::ERROR | &Level::WARN => EventFilter::Event,
                    &Level::INFO | &Level::DEBUG | &Level::TRACE => EventFilter::Breadcrumb,
                })
                .span_filter(|metadata| {
                    matches!(
                        metadata.level(),
                        &Level::ERROR | &Level::WARN | &Level::INFO | &Level::DEBUG
                    )
                }),
        );
    }

    let stdout_layer = if disable_stdout == Some(true) {
        None
    } else {
        Some(fmt::layer().with_ansi(atty::is(atty::Stream::Stdout)))
    };

    // Use env RUST_LOG to initialize log if present.
    // Otherwise, use the specified level.
    let directives = env::var(EnvFilter::DEFAULT_ENV).unwrap_or_else(|_x| level.to_string());
    let env_filter = EnvFilter::new(directives);
    let subscriber = Registry::default()
        .with(stdout_layer)
        .with(env_filter)
        .with(JsonStorageLayer)
        .with(file_logging_layer)
        .with(jaeger_layer)
        .with(sentry_layer);

    // For tokio-console
    #[cfg(feature = "console")]
    let subscriber = subscriber.with(console_subscriber::spawn());

    tracing::subscriber::set_global_default(subscriber)
        .expect("error setting global tracing subscriber");

    guards
}

pub fn init_query_logger(
    log_name: &str,
    dir: &str,
) -> (Vec<WorkerGuard>, Arc<dyn Subscriber + Send + Sync>) {
    let mut guards = vec![];

    let rolling_appender = RollingFileAppender::new(Rotation::HOURLY, dir, log_name);
    let (rolling_writer, rolling_writer_guard) = tracing_appender::non_blocking(rolling_appender);
    let format = tracing_subscriber::fmt::format()
        .with_ansi(atty::is(atty::Stream::Stdout))
        .without_time()
        .with_target(false)
        .with_level(false)
        .compact();
    guards.push(rolling_writer_guard);

    let subscriber = tracing_subscriber::fmt()
        .with_writer(rolling_writer)
        .event_format(format)
        .finish();

    (guards, Arc::new(subscriber))
}

/// Initialize unit test tracing for metasrv
pub fn init_meta_ut_tracing() {
    static START: Once = Once::new();

    START.call_once(|| {
        let mut g = META_UT_LOG_GUARD.as_ref().lock().unwrap();
        *g = Some(do_init_meta_ut_tracing(
            "unittest-meta",
            "./.databend/logs_unittest",
            "DEBUG",
        ));
    });
}

static META_UT_LOG_GUARD: Lazy<Arc<Mutex<Option<Vec<WorkerGuard>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(None)));

pub struct EventFormatter {}

impl<S, N> FormatEvent<S, N> for EventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let meta = event.metadata();

        SystemTime {}.format_time(&mut writer)?;
        writer.write_char(' ')?;

        let fmt_level = meta.level().as_str();
        write!(writer, "{:>5} ", fmt_level)?;

        write!(writer, "{:0>15?} ", std::thread::current().name())?;
        write!(writer, "{:0>2?} ", std::thread::current().id())?;

        if let Some(scope) = ctx.event_scope() {
            let mut seen = false;

            for span in scope.from_root() {
                write!(writer, "{}", span.metadata().name())?;
                write!(writer, "#{:x}", span.id().into_u64())?;

                seen = true;

                let ext = span.extensions();
                if let Some(fields) = &ext.get::<FormattedFields<N>>() {
                    if !fields.is_empty() {
                        write!(writer, "{{{}}}", fields)?;
                    }
                }
                write!(writer, ":")?;
            }

            if seen {
                writer.write_char(' ')?;
            }
        };

        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

pub fn do_init_meta_ut_tracing(app_name: &str, dir: &str, level: &str) -> Vec<WorkerGuard> {
    let mut guards = vec![];

    let span_rolling_appender = RollingFileAppender::new(Rotation::HOURLY, dir, app_name);
    let (writer, writer_guard) = tracing_appender::non_blocking(span_rolling_appender);

    let f_layer = fmt::Layer::new()
        .with_span_events(fmt::format::FmtSpan::FULL)
        .with_writer(writer)
        .with_ansi(false)
        .event_format(EventFormatter {});

    guards.push(writer_guard);

    // Use env RUST_LOG to initialize log if present.
    // Otherwise use the specified level.
    let directives = env::var(EnvFilter::DEFAULT_ENV).unwrap_or_else(|_x| level.to_string());
    let env_filter = EnvFilter::new(directives);
    let subscriber = Registry::default().with(env_filter).with(f_layer);

    tracing::subscriber::set_global_default(subscriber)
        .expect("error setting global tracing subscriber");

    guards
}
