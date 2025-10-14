use std::fmt::Debug;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::exit;

use fs2::FileExt;

use anyhow::anyhow;
use chrono::NaiveDateTime;
use clap::{Parser, ValueEnum};
use lettre::message::Mailboxes;
use lettre::message::header::{self, ContentType, UserAgent};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use serde::{Deserialize, Serialize};
use strum_macros::{Display as EnumDisplay, EnumString};
use tera::{Context, Tera};
use time::macros::format_description;
use tracing::{debug, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::time::LocalTime;

use aws_config::BehaviorVersion;
use aws_config::SdkConfig;
use aws_sdk_sns as sns;

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

    /// Dry Run
    #[clap(long)]
    dry_run: bool,

    /// Method
    #[clap(short, default_value = "smtp")]
    method: Option<Method>,

    /// SNS Topic (required when method is sns)
    #[clap(short = 'T')]
    topic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    smtp: Smtp,
    sns: Option<Sns>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Smtp {
    host: String,
    user_name: String,
    password: String,
    from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sns {
    aws_account_id: String,
    aws_profile: String,
    aws_region: String,
}

#[derive(ValueEnum, Debug, Copy, Clone, PartialEq, Eq, Serialize)]
enum Method {
    #[value(rename_all = "LOWER")]
    Smtp,
    #[value(rename_all = "LOWER")]
    Sns,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::fs::create_dir_all("./log")?;

    let log_file = open_log_file()?;

    tracing_subscriber::fmt()
        .with_timer(LocalTime::new(format_description!(
            "[year]-[month]-[day]T[hour repr:24]:[minute]:[second].[subsecond digits:6]Z"
        )))
        .with_writer(log_file)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    debug!("started");

    let args = Args::parse();

    // Validate SNS method requires topic
    if args.method == Some(Method::Sns) && args.topic.is_none() {
        error!("--topic is required when using --method sns");
        exit(1);
    }

    let config = load_config().expect("failed to load config file");

    // Validate SNS configuration
    if args.method == Some(Method::Sns) {
        if let Some(sns_config) = &config.sns {
            if sns_config.aws_account_id.is_empty() {
                error!("sns.aws_account_id must not be empty in config");
                exit(1);
            }
            if sns_config.aws_profile.is_empty() {
                error!("sns.aws_profile must not be empty in config");
                exit(1);
            }
            if sns_config.aws_region.is_empty() {
                error!("sns.aws_region must not be empty in config");
                exit(1);
            }
        } else {
            error!("sns configuration is required when using --method sns");
            exit(1);
        }
    }

    if let Some(method) = args.method {
        if method == Method::Smtp {
            let body = create_body(&args).expect("failed to create mail body");

            if args.verbose {
                println!("{}", &body);
            }

            if args.dry_run {
                info!("Dry run mode enabled, not sending email.");
                exit(0);
            }

            match send_mail(&config, &args, &body) {
                Ok(()) => exit(0),
                Err(_e) => exit(1),
            }
        } else if method == Method::Sns {
            // let body = create_body(&args).expect("failed to create mail body");
            let body = subject(&args);

            if args.verbose {
                println!("{}", &body);
            }

            if args.dry_run {
                info!("Dry run mode enabled, not sending sns.");
                exit(0);
            }

            match push_sns(&config, &args, &body).await {
                Ok(()) => exit(0),
                Err(_e) => exit(1),
            }
        }
    }

    Ok(())
}

fn open_log_file() -> std::io::Result<File> {
    let path = Path::new("./log/notify.log");

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o660) // no effect due to umask??
        .open(path)?;

    // Acquire exclusive lock on the log file
    file.lock_exclusive()?;

    Ok(file)
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
            println!("Parsing error(s): {e}");
            exit(1);
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

async fn push_sns(config: &Config, args: &Args, message: &str) -> anyhow::Result<()> {
    let aws_config = load_aws_config(config).await?;
    let client = sns::Client::new(&aws_config);

    let topic = args
        .topic
        .as_ref()
        .ok_or_else(|| anyhow!("topic is required when using SNS method"))?;

    let sns_config = config
        .sns
        .as_ref()
        .ok_or_else(|| anyhow!("SNS configuration not found"))?;

    let topic_arn = format!(
        "arn:aws:sns:{}:{}:{}",
        sns_config.aws_region, sns_config.aws_account_id, topic
    );

    debug!("topic arn: {}", topic_arn);

    match client
        .publish()
        .topic_arn(topic_arn)
        .message(message)
        .send()
        .await
    {
        Ok(out) => {
            info!(
                "SNS sent successfully: {}",
                out.message_id().unwrap_or_default()
            );
            Ok(())
        }
        Err(e) => {
            error!("Could not send SNS: {e:?}");
            Err(e.into())
        }
    }
}

async fn load_aws_config(config: &Config) -> anyhow::Result<SdkConfig> {
    let profile = config
        .sns
        .as_ref()
        .map(|s| s.aws_profile.as_str())
        .unwrap_or("default");

    debug!("aws profile: {}", profile);

    let loader = aws_config::defaults(BehaviorVersion::latest()).profile_name(profile);

    Ok(loader.load().await)
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
