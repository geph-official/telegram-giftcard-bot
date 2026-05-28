use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use acidjson::AcidJson;
use anyhow::Context;
use argh::FromArgs;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use teloxide::{
    dispatching::UpdateFilterExt,
    payloads::SendMessageSetters,
    prelude::*,
    types::{ChatId, Message, ReplyParameters, User, UserId},
};

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

const MSG_RECIPIENT_COUNT: &str = "🌸 {count} users received giftcards!";
const MSG_ALREADY_REDEEMED: &str = "🎁 You have already received a giftcard! Each user will only receive 1 giftcard\n\n🧧 您已经获得了一张礼品卡！每名用户可以得到一张礼品卡";
const MSG_CONGRATS: &str = "🎉 Congratulations! Here's a 3-day Geph Plus giftcard for you:\n\n恭喜您！这里是一张3天迷雾通 Plus 礼品卡:";
const MSG_REDEEM_STEPS: &str = "💳 To redeem the giftcard: open the Geph app --> \"Buy Plus\" / \"Extend\" in the top right corner --> \"Redeem voucher\"\n\n💝 如何兑换礼品卡：打开迷雾通 APP --> 点击右上角的“购买 Plus”或“延长” --> “兑换礼品卡”";
const MSG_JOIN_GROUP: &str = "⛔ You must join our official group to get a giftcard:\n🚦 您必须加入迷雾通官方群组才能获得礼品卡： https://t.me/gephusers";
const MSG_MEMBERSHIP_CHECK_FAILED: &str = "⚠️ I couldn't verify your group membership right now. Please try again later.\n\n⚠️ 暂时无法验证您的群组成员身份。请稍后重试。";
const MSG_GROUP_REPLY: &str = "Please private message https://t.me/GephGiftcardBot to get your giftcard\n\n请私信 https://t.me/GephGiftcardBot 来领取礼品卡\n\nلطفاً برای دریافت گیفت‌کارت به من پیام خصوصی بدهید: https://t.me/GephGiftcardBot";

static STORE: Lazy<AcidJson<Store>> = Lazy::new(|| {
    AcidJson::open_or_else(Path::new(&CONFIG.store_path), || Store {
        redeemed_users: BTreeSet::new(),
    })
    .unwrap()
});

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Lazy::force(&CONFIG);
    Lazy::force(&STORE);

    let bot = Bot::new(CONFIG.telegram_token.clone());
    let handler = Update::filter_message().endpoint(dispatch_message);

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn dispatch_message(bot: Bot, msg: Message) -> ResponseResult<()> {
    if let Err(err) = handle_message(bot, msg).await {
        eprintln!("failed to process message: {err:?}");
    }

    Ok(())
}

async fn handle_message(bot: Bot, msg: Message) -> anyhow::Result<()> {
    let Some(sender) = msg.from.clone() else {
        return Ok(());
    };
    let text = msg.text().unwrap_or_default().to_owned();

    if msg.chat.is_private() {
        handle_private_message(&bot, &msg, &sender, &text).await?;
    } else if msg.chat.is_group() || msg.chat.is_supergroup() {
        handle_group_message(&bot, &msg, &text).await?;
    }

    Ok(())
}

async fn handle_private_message(
    bot: &Bot,
    msg: &Message,
    sender: &User,
    text: &str,
) -> anyhow::Result<()> {
    let chat_id = msg.chat.id;
    let sender_uname = sender.username.clone().unwrap_or_default();
    let sender_id: i64 = sender
        .id
        .0
        .try_into()
        .context("sender id does not fit into i64")?;

    if sender_uname == CONFIG.admin_uname {
        if text == "#RecipientCount" {
            let count = STORE.read().redeemed_users.len();
            let msg = MSG_RECIPIENT_COUNT.replace("{count}", &count.to_string());
            bot.send_message(chat_id, msg).await?;
        }
        return Ok(());
    }

    if STORE.read().redeemed_users.contains(&sender_id) {
        bot.send_message(chat_id, MSG_ALREADY_REDEEMED).await?;
        return Ok(());
    }

    let group_id = ChatId(CONFIG.geph_group_id);

    match user_in_group(bot, sender.id, group_id).await {
        Ok(true) => {
            let gc =
                create_giftcards(CONFIG.days_per_giftcard, &CONFIG.create_giftcard_secret).await?;
            STORE.write().redeemed_users.insert(sender_id);

            bot.send_message(chat_id, MSG_CONGRATS).await?;
            bot.send_message(chat_id, &gc).await?;
            bot.send_message(chat_id, MSG_REDEEM_STEPS).await?;
        }
        Ok(false) => {
            bot.send_message(chat_id, MSG_JOIN_GROUP).await?;
        }
        Err(err) => {
            eprintln!("failed to check group membership for user {sender_id}: {err:?}");
            bot.send_message(chat_id, MSG_MEMBERSHIP_CHECK_FAILED)
                .await?;
        }
    }

    Ok(())
}

async fn handle_group_message(bot: &Bot, msg: &Message, text: &str) -> anyhow::Result<()> {
    let bot_mention = format!("@{}", CONFIG.bot_uname);
    if text.contains(&bot_mention) {
        bot.send_message(msg.chat.id, MSG_GROUP_REPLY)
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
    }

    Ok(())
}

async fn user_in_group(bot: &Bot, user_id: UserId, group_id: ChatId) -> anyhow::Result<bool> {
    let member = bot
        .get_chat_member(group_id, user_id)
        .await
        .with_context(|| format!("get_chat_member failed for user {}", user_id.0))?;

    Ok(member.is_present())
}

pub async fn create_giftcards(days: u32, secret: &str) -> Result<String, reqwest::Error> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let body = json!({
        "days_per_card": days,
        "num_cards": 1,
        "secret": secret,
    });

    let response = client
        .post("https://web-backend.geph.io/support/create-giftcards")
        .json(&body)
        .send()
        .await?
        .text()
        .await?;

    Ok(response.trim().to_string())
}
