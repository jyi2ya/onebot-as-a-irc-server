use std::fmt::Debug;
use std::sync::Arc;

use futures::{Sink, SinkExt, Stream, StreamExt};
use irc_proto::{Command, Message, Prefix, Response};
use kovi::log;
use kovi_broadcast_plugin::kovi::tokio;
use kovi_broadcast_plugin::{RenderedOnebotMessage, kovi};
use tokio::sync::Mutex;
use tokio_util::codec::Decoder;

fn prefixed(prefix: Prefix, mut message: Message) -> Message {
    message.prefix = Some(prefix);
    message
}

async fn handle_irc_messages<IN, OUT, E>(
    mut irc_rx: IN,
    irc_tx: Arc<Mutex<OUT>>,
    bot: Arc<kovi::RuntimeBot>,
    quit_notifier: tokio::sync::oneshot::Sender<()>,
) where
    IN: Stream<Item = Result<Message, E>> + Unpin,
    OUT: Sink<Message> + Unpin,
    E: Debug,
    <OUT as futures::Sink<Message>>::Error: Debug,
{
    let mut nick = "".to_owned();
    let mut nick_prefix = Prefix::Nickname("".to_owned(), "".to_owned(), "".to_owned());
    let server_prefix = Prefix::ServerName("onebotirc.villv.tech".to_owned());
    irc_tx
        .lock()
        .await
        .send(prefixed(
            server_prefix.clone(),
            Message::from(Command::Response(
                Response::RPL_WELCOME,
                vec!["rivus".to_owned(), "Welcome To QQ Bridge".to_owned()],
            )),
        ))
        .await
        .unwrap();
    while let Some(next_frame) = irc_rx.next().await {
        let message = match next_frame {
            Ok(message) => message,
            Err(err) => {
                dbg!(err);
                continue;
            }
        };

        eprintln!("from user: {message}");
        let reply = match message.command {
            Command::CAP(_, _, _, _) => Vec::new(),
            Command::JOIN(chanlist, chankeys, realname) => vec![
                prefixed(
                    nick_prefix.clone(),
                    Message::from(Command::JOIN(chanlist.clone(), chankeys, realname)),
                ),
                prefixed(
                    server_prefix.clone(),
                    Message::from(Command::Response(
                        Response::RPL_TOPIC,
                        vec![chanlist.clone(), "TOPIC not implemented yet".to_owned()],
                    )),
                ),
                prefixed(
                    server_prefix.clone(),
                    Message::from(Command::Response(
                        Response::RPL_NAMREPLY,
                        vec![chanlist.clone(), nick.clone()],
                    )),
                ),
                prefixed(
                    server_prefix.clone(),
                    Message::from(Command::Response(
                        Response::RPL_ENDOFNAMES,
                        vec![chanlist.clone(), "end of list".to_owned()],
                    )),
                ),
            ],
            // Some(Message::from(Command::TOPIC( "TOPIC not implemented".to_owned(), None,))),
            Command::ChannelMODE(_modes, _modeparams) => Vec::new(),
            Command::UserMODE(_nickname, _modes) => Vec::new(),
            Command::WHOIS(_target, _masklist) => vec![prefixed(
                server_prefix.clone(),
                Message::from(Command::Response(Response::RPL_WHOISUSER, Vec::new())),
            )],
            Command::PING(server1, server2) => vec![Message::from(Command::PONG(server1, server2))],
            Command::PRIVMSG(target, message) => {
                if let Some(stripped) = target.strip_prefix('#') {
                    let group_id = stripped.parse().unwrap();
                    bot.send_group_msg(group_id, message);
                } else {
                    let peer_id = target
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>()
                        .parse()
                        .unwrap();
                    bot.send_private_msg(peer_id, message);
                }
                Vec::new()
            }
            Command::QUIT(_message) => {
                break;
            }
            Command::NICK(nickname) => {
                nick = nickname;
                nick_prefix = Prefix::Nickname(nick.clone(), "".to_owned(), "".to_owned());
                Vec::new()
            }
            Command::USER(_user, _mode, _realname) => Vec::new(),
            Command::USERHOST(nicknames) => vec![prefixed(
                server_prefix.clone(),
                Message::from(Command::Response(Response::RPL_USERHOST, nicknames)),
            )],
            _ => {
                dbg!(message);
                unimplemented!();
            }
        };

        for msg in reply {
            irc_tx.lock().await.feed(msg).await.unwrap();
        }
        irc_tx.lock().await.flush().await.unwrap();
    }
    quit_notifier.send(()).unwrap();
}

async fn handle_onebot_messages<OUT>(
    mut onebot_rx: tokio::sync::broadcast::Receiver<RenderedOnebotMessage>,
    irc_tx: Arc<Mutex<OUT>>,
    quit_signal: tokio::sync::oneshot::Receiver<()>,
) where
    OUT: Sink<Message> + Unpin,
    <OUT as futures::Sink<irc_proto::Message>>::Error: std::fmt::Debug,
{
    while quit_signal.is_empty() {
        let message = onebot_rx.recv().await.unwrap();
        let message = match message {
            RenderedOnebotMessage::Group {
                content,
                sender_id,
                sender_name,
                group_id,
            } => Message::new(
                Some(format!("{sender_id}({sender_name})").as_str()),
                "PRIVMSG",
                vec![format!("#{group_id}").as_str(), content.as_str()],
            )
            .unwrap(),
            RenderedOnebotMessage::Private {
                content,
                sender_id,
                sender_name,
            } => Message::new(
                Some(format!("{sender_id}({sender_name})").as_str()),
                "PRIVMSG",
                vec![sender_id.to_string().as_str(), content.as_str()],
            )
            .unwrap(),
        };
        eprintln!("from onebot: {message}");
        irc_tx.lock().await.send(message).await.unwrap();
    }
    log::info!("onebot message handler quit");
}

async fn irc_server_main(
    bind_addr: std::net::SocketAddr,
    broadcast_tx: tokio::sync::broadcast::Sender<RenderedOnebotMessage>,
) {
    let acceptor = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    while let Ok((conn, peer)) = acceptor.accept().await {
        log::info!("incoming connection: {peer}");
        let codec = irc_proto::IrcCodec::new("irc_codec").unwrap();
        let irc_conn = codec.framed(conn);
        let (irc_tx, irc_rx) = irc_conn.split();
        let onebot_rx = broadcast_tx.subscribe();
        let bot = loop {
            if let Some(bot) = kovi_broadcast_plugin::RUNTIME_BOT.get() {
                break bot.clone();
            }
            tokio::task::yield_now().await;
        };

        let irc_tx = Arc::new(Mutex::new(irc_tx));
        let (quit_notifier, quit_signal) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(handle_irc_messages(
            irc_rx,
            irc_tx.clone(),
            bot,
            quit_notifier,
        ));
        tokio::spawn(handle_onebot_messages(
            onebot_rx,
            irc_tx.clone(),
            quit_signal,
        ));
    }
}

fn main() {
    let bind_addr = "0.0.0.0:8621".parse().unwrap();

    let (broadcast_tx, _broadcast_rx) = tokio::sync::broadcast::channel(16);
    kovi_broadcast_plugin::RENDERED_MESSAGE_CHANNEL
        .set(broadcast_tx.clone())
        .unwrap();
    let mut bot = kovi::build_bot!(kovi_broadcast_plugin);
    bot.spawn(irc_server_main(bind_addr, broadcast_tx));
    bot.run();
}
