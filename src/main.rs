use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use acidjson::AcidJson;
use anyhow::Context;
use argh::FromArgs;
use async_compat::CompatExt;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use telegram_bot::{Response, TelegramBot};

/// configuration yaml file for geph telegram giftcard bot
#[derive(FromArgs, PartialEq, Debug)]
struct Args {
    /// configuration yaml file path
    #[argh(option, short = 'c', long = "config")]
    config: PathBuf,
}

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    store_path: String,
    telegram_token: String,
    admin_uname: String,
    bot_uname: String,
    geph_group_id: i64,
    create_giftcard_secret: String,
    days_per_giftcard: u32,
}

static ARGS: Lazy<Args> = Lazy::new(argh::from_env);

static CONFIG: Lazy<Config> = Lazy::new(|| {
    let s = &std::fs::read(&ARGS.config).expect("cannot read config file");
    serde_yaml::from_slice(s).expect("cannot parse config file")
});

#[derive(Serialize, Deserialize, Clone)]
struct Store {
    redeemed_users: BTreeSet<i64>,
}

static STORE: Lazy<AcidJson<Store>> = Lazy::new(|| {
    AcidJson::open_or_else(Path::new(&CONFIG.store_path), || Store {
        redeemed_users: BTreeSet::new(),
    })
    .unwrap()
});

static TELEGRAM: Lazy<TelegramBot> =
    Lazy::new(|| TelegramBot::new(&CONFIG.telegram_token, telegram_msg_handler));

async fn user_in_group(user_id: i64, group_id: i64) -> anyhow::Result<bool> {
    let res = TELEGRAM
        .call_api(
            "getChatMember",
            json!({ "chat_id": group_id, "user_id": user_id }),
        )
        .await;
    match res {
        Ok(member_info) => {
            let status = member_info["status"].as_str().unwrap_or_default();
            Ok(matches!(status, "member" | "administrator" | "creator"))
        }
        Err(_) => Ok(false),
    }
}

async fn telegram_msg_handler(update: Value) -> anyhow::Result<Vec<Response>> {
    let admin_uname = &CONFIG.admin_uname;
    let sender_id = update["message"]["from"]["id"]
        .as_i64()
        .context("could not get sender id")?;
    let msg = update["message"]["text"].as_str().unwrap_or_default();
    let sender_uname = update["message"]["from"]["username"]
        .as_str()
        .unwrap_or_default();

    if update["message"]["chat"]["type"].as_str() == Some("private") {
        println!("from: uname={sender_uname}, id={sender_id}");
        if sender_uname == admin_uname {
            if msg == "#RecipientCount" {
                let count = STORE.read().redeemed_users.len();
                return to_response(&format!("🌸 {count} users received giftcards!"), update);
            }
        } else {
            if STORE.read().redeemed_users.contains(&sender_id) {
                return to_response(
                    "🎁 You have already received a giftcard! Each user will only receive 1 giftcard\n\n🧧 您已经获得了一张礼品卡！每名用户可以得到一张礼品卡",
                    update,
                );
            }

            if user_in_group(sender_id, CONFIG.geph_group_id).await? {
                let gc = create_giftcards(CONFIG.days_per_giftcard, &CONFIG.create_giftcard_secret)
                    .await?;
                STORE.write().redeemed_users.insert(sender_id);

                TELEGRAM
                        .send_msg(Response {
                            text: format!(
                                "🎉 Congratulations! Here's a 1-day Geph Plus giftcard for you:\n\n恭喜您！这里是一张1天迷雾通 Plus 礼品卡:"
                            ),
                            chat_id: sender_id,
                            reply_to_message_id: None,
                        })
                        .await?;
                TELEGRAM
                    .send_msg(Response {
                        text: gc,
                        chat_id: sender_id,
                        reply_to_message_id: None,
                    })
                    .await?;
                return to_response("💳 To redeem the giftcard: open the Geph app --> \"Buy Plus\" / \"Extend\" in the top right corner --> \"Redeem voucher\"\n\n💝 如何兑换礼品卡：打开迷雾通 APP --> 点击右上角的“购买 Plus”或“延长” --> “兑换礼品卡”".into(),
update);
            } else {
                return to_response(
                    "⛔ You must join our official group to get a giftcard:\n🚦 您必须加入迷雾通官方群组才能获得礼品卡： https://t.me/gephusers",
                    update,
                );
            }
        }
    }
    Ok(vec![])
}

pub async fn create_giftcards(days: u32, secret: &str) -> Result<String, reqwest::Error> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let body = json!({
        "days_per_card": days,
        "num_cards": 3,
        "secret": secret,
    });

    let response = client
        .post("https://web-backend.geph.io/support/create-giftcards")
        .json(&body)
        .send()
        .await?
        .text()
        .await?;

    let code = response.trim().to_string();

    Ok(code)
}

fn to_response(text: &str, responding_to: Value) -> anyhow::Result<Vec<Response>> {
    Ok(vec![Response {
        text: text.to_owned(),
        chat_id: responding_to["message"]["chat"]["id"]
            .as_i64()
            .context("could not get chat id")?,
        reply_to_message_id: None,
    }])
}

fn main() {
    Lazy::force(&TELEGRAM);
    loop {
        std::thread::park();
    }
}
