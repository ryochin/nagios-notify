use std::fmt::Debug;

use chrono::NaiveDateTime;
use clap::{Parser, ValueEnum};
use lettre::message::header::{self, ContentType, UserAgent};
use lettre::message::Mailboxes;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use serde::{Deserialize, Serialize};
use strum_macros::{Display as EnumDisplay, EnumString};
use tera::{Context, Tera};
use time::macros::format_description;
use tracing::{debug, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::time::LocalTime;

/// Nagios Notify
#[derive(Parser, Debug, Clone, Serialize)]
#[clap(version, about, next_line_help = true)]
struct Args {
    /// Verbose output
    #[clap(short, long)]
    verbose: bool,

    /// Host name
    #[clap(short = 'H')]
    host: String,

    /// Addresses
    #[clap(short)]
    addresses: String,

    /// Host Address
    #[clap(short = 'A')]
    host_address: Option<String>,

    /// Type
    #[clap(short)]
    r#type: EventType,

    /// Datetime
    #[clap(short)]
    datetime: String,

    /// Notification Type
    #[clap(short)]
    notification_type: NotificationType,

    /// Service
    #[clap(short)]
    service: Option<String>,

    /// Status
    #[clap(short = 'S')]
    status: Option<Status>,

    /// Service Output
    #[clap(short)]
    output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    smtp: Smtp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Smtp {
    host: String,
    user_name: String,
    password: String,
    from: String,
}

#[derive(ValueEnum, Debug, Copy, Clone, PartialEq, Eq, Serialize)]
enum EventType {
    #[value(rename_all = "LOWER")]
    Host,
    #[value(rename_all = "LOWER")]
    Service,
}

#[derive(ValueEnum, Debug, Copy, Clone, PartialEq, Eq, Serialize, EnumDisplay, EnumString)]
enum NotificationType {
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "PROBLEM")]
    Problem,
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "RECOVERY")]
    Recovery,
}

#[derive(ValueEnum, Debug, Copy, Clone, PartialEq, Eq, Serialize, EnumDisplay, EnumString)]
enum Status {
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "OK")]
    Ok,
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "WARNING")]
    Warning,
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "CRITICAL")]
    Critical,
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "UNKNOWN")]
    Unknown,
    #[value(rename_all = "UPPER")]
    #[strum(serialize = "UNREACHABLE")]
    Unreachable,
}

fn main() {
    let file_appender = tracing_appender::rolling::daily("./log", "notify.log");

    tracing_subscriber::fmt()
        .with_timer(LocalTime::new(format_description!(
            "[year]-[month]-[day]T[hour repr:24]:[minute]:[second].[subsecond digits:6]Z"
        )))
        .with_writer(file_appender)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    debug!("started");

    let args = Args::parse();

    let config = load_config().expect("failed to load config file");

    let body = create_body(&args).expect("failed to create mail body");

    if args.verbose {
        println!("{}", &body);
    }

    match send_mail(&config, &args, &body) {
        Ok(()) => ::std::process::exit(0),
        Err(_e) => ::std::process::exit(1),
    }
}

fn load_config() -> Result<Config, String> {
    let str = std::fs::read_to_string("config.yml")
        .map_err(|_| "failed to load config file".to_string())?;

    serde_yaml::from_str(&str).map_err(|_| "".to_string())
}

fn create_body(args: &Args) -> tera::Result<String> {
    let tera = match Tera::new("./*.txt") {
        Ok(t) => t,
        Err(e) => {
            println!("Parsing error(s): {}", e);
            ::std::process::exit(1);
        }
    };

    let host_address = &args.clone().host_address.unwrap_or_else(|| "?".to_string());

    let mut context = Context::new();
    context.insert("args", args);
    context.insert("title", &title(args));
    context.insert("datetime", &datetime(args));
    context.insert("monitor", &monitor());
    context.insert("host_address", host_address);
    context.insert("status_description", &status_description(args));

    tera.render("template.txt", &context)
}

fn send_mail(
    config: &Config,
    args: &Args,
    body: &str,
) -> Result<(), lettre::transport::smtp::Error> {
    let from = config.smtp.from.clone();

    let mailboxes: Mailboxes = args.addresses.parse().unwrap();
    let to_header: header::To = mailboxes.into();

    let subject = subject(args);

    info!("subject: {}", subject);

    let email = Message::builder()
        .from(from.parse().unwrap())
        .reply_to(from.parse().unwrap())
        .mailbox(to_header)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .header(mailer_name())
        .body(String::from(body))
        .unwrap();

    let creds = Credentials::new(
        config.smtp.user_name.to_owned(),
        config.smtp.password.to_owned(),
    );

    debug!("relay host: {}", &config.smtp.host);

    let mailer = SmtpTransport::relay(&config.smtp.host)
        .unwrap()
        .credentials(creds)
        .build();

    match mailer.send(&email) {
        Ok(_) => {
            info!("Email sent successfully");
            Ok(())
        }
        Err(e) => {
            error!("Could not send email: {e:?}");
            Err(e)
        }
    }
}

fn mailer_name() -> UserAgent {
    UserAgent::from(format!("Nagios Notify/{}", env!("CARGO_PKG_VERSION")))
}

fn datetime(args: &Args) -> String {
    NaiveDateTime::parse_from_str(&args.datetime, "%a %b %d %H:%M:%S %Z %Y")
        .ok()
        .map(|d| d.format("%m月%d日 %H時%M分").to_string())
        .unwrap_or_else(|| "不明".to_string())
}

fn monitor() -> String {
    let default = "localhost".to_string();

    hostname::get().map_or(default.clone(), |s| s.into_string().unwrap_or(default))
}

fn title(args: &Args) -> String {
    if args.notification_type == NotificationType::Problem
        || args.notification_type == NotificationType::Recovery
    {
        format!(
            "{}{}",
            title_type_name(args),
            title_status_description(args)
        )
    } else {
        "何らかの問題が発生（詳細不明）".to_string()
    }
}

fn title_type_name(args: &Args) -> &str {
    if is_host(args) {
        "ホスト"
    } else {
        "サービス"
    }
}

fn title_status_description(args: &Args) -> &str {
    if args.notification_type == NotificationType::Recovery {
        "が正常状態に復帰"
    } else {
        "に問題が発生"
    }
}

fn subject(args: &Args) -> String {
    if is_host(args) {
        format!(
            "[{}] {}: {}    by {}",
            args.notification_type,
            host_status(args),
            args.host,
            monitor()
        )
    } else {
        format!(
            "[{}] {}: {}/{}    by {}",
            args.notification_type,
            args.status.unwrap_or(Status::Unknown),
            args.host,
            args.service.clone().unwrap_or("?".to_string()),
            monitor()
        )
    }
}

fn host_status(args: &Args) -> &str {
    match args.notification_type {
        NotificationType::Problem => "DOWN",
        NotificationType::Recovery => "UP",
    }
}

fn status_description(args: &Args) -> Option<&str> {
    if is_host(args) {
        args.status.map(|s| match s {
            Status::Ok => "回復",
            Status::Warning => "警告",
            Status::Critical => "ダウン",
            Status::Unknown => "不明",
            Status::Unreachable => "到達不能（経路障害の可能性）",
        })
    } else {
        args.status.map(|s| match s {
            Status::Ok => "回復",
            Status::Warning => "警告",
            Status::Critical => "致命的",
            Status::Unknown => "不明",
            Status::Unreachable => "到達不能（経路障害の可能性）",
        })
    }
}

fn is_host(args: &Args) -> bool {
    args.r#type == EventType::Host
}
