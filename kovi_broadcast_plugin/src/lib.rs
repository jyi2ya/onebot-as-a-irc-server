pub use kovi;

use kovi::tokio;

#[derive(Debug, Clone)]
pub enum RenderedOnebotMessage {
    Group {
        content: String,
        sender_id: i64,
        sender_name: String,
        group_id: i64,
    },
    Private {
        content: String,
        sender_id: i64,
        sender_name: String,
    },
}

impl From<&kovi::MsgEvent> for RenderedOnebotMessage {
    fn from(value: &kovi::MsgEvent) -> Self {
        if value.is_private() {
            Self::Private {
                content: value.get_text(),
                sender_id: value.sender.user_id,
                sender_name: value.get_sender_nickname(),
            }
        } else {
            Self::Group {
                content: value.get_text(),
                sender_id: value.sender.user_id,
                sender_name: value.get_sender_nickname(),
                group_id: value.group_id.unwrap(),
            }
        }
    }
}

pub static RENDERED_MESSAGE_CHANNEL: std::sync::OnceLock<
    tokio::sync::broadcast::Sender<RenderedOnebotMessage>,
> = std::sync::OnceLock::new();

pub static RUNTIME_BOT: std::sync::OnceLock<std::sync::Arc<kovi::RuntimeBot>> =
    std::sync::OnceLock::new();

#[kovi::plugin]
async fn render_and_push_qq_messages() {
    let bot = kovi::PluginBuilder::get_runtime_bot();
    let _ = RUNTIME_BOT.set(bot); // TODO: bot not Debug
    kovi::PluginBuilder::on_msg(|event| async move {
        RENDERED_MESSAGE_CHANNEL
            .get()
            .unwrap()
            .send(RenderedOnebotMessage::from(event.as_ref()))
            .unwrap();
    })
}
