use std::fmt::Debug;
use std::sync::Arc;

use anyhow::Context;
use futures::{Sink, SinkExt, Stream, StreamExt};
use irc_proto::{Command, Message, Prefix, Response};
use kovi::log;
use kovi_broadcast_plugin::kovi::tokio;
use kovi_broadcast_plugin::{RenderedOnebotMessage, kovi};
use tokio::sync::Mutex;
use tokio_util::codec::Decoder;
use tokio_util::sync::CancellationToken;

fn prefixed(prefix: Prefix, mut message: Message) -> Message {
    message.prefix = Some(prefix);
    message
}

fn parse_prefix_number<N>(value: &str) -> Option<N>
where
    N: std::str::FromStr,
    <N as std::str::FromStr>::Err: std::fmt::Debug,
{
    value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

async fn handle_irc_messages<IN, OUT, E>(
    mut irc_rx: IN,
    irc_tx: Arc<Mutex<OUT>>,
    bot: Arc<kovi::RuntimeBot>,
    cancel_signal: CancellationToken,
) -> anyhow::Result<()>
where
    IN: Stream<Item = Result<Message, E>> + Unpin,
    OUT: Sink<Message> + Unpin,
    E: Debug,
    <OUT as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + Debug + 'static,
{
    let mut nick = "rivus".to_owned();
    let mut nick_prefix = Prefix::Nickname(
        "rivus".to_owned(),
        "rivus".to_owned(),
        "villv.tech".to_owned(),
    );
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
    loop {
        let next_item = tokio::select! {
            _ = cancel_signal.cancelled() => {
                return Ok(());
            },
            next_item = irc_rx.next() => next_item,
        };

        let next_frame = match next_item {
            Some(frame) => frame,
            None => {
                log::info!("irc client closed");
                return Ok(());
            }
        };

        let message = match next_frame {
            Ok(message) => message,
            Err(err) => {
                log::warn!("invaid irc message received: {:?}", err);
                continue;
            }
        };

        eprintln!("from user: {message}");
        let reply = match message.command {
            Command::CAP(_, _, _, _) => Vec::new(),
            Command::WHO(_name, _is_op) => Vec::new(),
            Command::JOIN(chanlist, _chankeys, _realname) => {
                let mut reply = Vec::new();
                for channel in chanlist.split(',') {
                    let group_id = if let Some(id) = parse_prefix_number(channel) {
                        id
                    } else {
                        continue;
                    };
                    let group_name = match bot.get_group_info(group_id, false).await {
                        Ok(resp) => resp.data["group_name"].as_str().unwrap().to_owned(),
                        Err(_) => "无法获取群名".to_owned(),
                    };
                    let reply_messages = vec![
                        prefixed(
                            nick_prefix.clone(),
                            Message::from(Command::JOIN(channel.to_owned(), None, None)),
                        ),
                        prefixed(
                            server_prefix.clone(),
                            Message::from(Command::Response(
                                Response::RPL_TOPIC,
                                vec![nick.clone(), channel.to_owned(), group_name],
                            )),
                        ),
                        prefixed(
                            server_prefix.clone(),
                            Message::from(Command::Response(
                                Response::RPL_NAMREPLY,
                                vec![
                                    nick.clone(),
                                    channel.to_owned(),
                                    format!("{nick}@villv.tech"),
                                ],
                            )),
                        ),
                        prefixed(
                            server_prefix.clone(),
                            Message::from(Command::Response(
                                Response::RPL_ENDOFNAMES,
                                vec![nick.clone(), channel.to_owned(), "end of list".to_owned()],
                            )),
                        ),
                    ];
                    reply.extend(reply_messages);
                }
                reply
            }
            Command::ChannelMODE(_modes, _modeparams) => Vec::new(),
            Command::UserMODE(_nickname, _modes) => Vec::new(),
            Command::WHOIS(_target, _masklist) => vec![prefixed(
                server_prefix.clone(),
                Message::from(Command::Response(
                    Response::RPL_WHOISUSER,
                    vec![nick.clone()],
                )),
            )],
            Command::PING(server1, server2) => vec![Message::from(Command::PONG(server1, server2))],
            Command::PRIVMSG(target, message) => {
                if let Some(stripped) = target.strip_prefix('#') {
                    let group_id = parse_prefix_number(stripped).unwrap();
                    bot.send_group_msg(group_id, message);
                } else {
                    let peer_id = parse_prefix_number(&target).unwrap();
                    bot.send_private_msg(peer_id, message);
                }
                Vec::new()
            }
            Command::QUIT(_message) => {
                break;
            }
            Command::PART(_chanlist, _comment) => Vec::new(),
            Command::NICK(nickname) => {
                nick = nickname;
                nick_prefix =
                    Prefix::Nickname(nick.clone(), "rivus".to_owned(), "villv.tech".to_owned());
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

        eprintln!(
            "server reply: {}",
            reply
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join("")
        );
        for msg in reply {
            irc_tx
                .lock()
                .await
                .feed(msg)
                .await
                .context("irc connection broken")?;
        }
        irc_tx
            .lock()
            .await
            .flush()
            .await
            .context("irc connection broken")?;
    }

    Ok(())
}

async fn handle_onebot_messages<OUT>(
    mut onebot_rx: tokio::sync::broadcast::Receiver<RenderedOnebotMessage>,
    irc_tx: Arc<Mutex<OUT>>,
    cancel_signal: CancellationToken,
) -> anyhow::Result<()>
where
    OUT: Sink<Message> + Unpin,
    <OUT as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + Debug + 'static,
{
    loop {
        let recv_item = tokio::select! {
            _ = cancel_signal.cancelled() => {
                return Ok(());
            }
            recv_item = onebot_rx.recv() => recv_item,
        };

        let message = recv_item.context("onebot closed")?;
        let messages: Vec<_> = match message {
            RenderedOnebotMessage::Group {
                content,
                sender_name,
                group_id,
                sender_id,
                ..
            } => {
                let nick_prefix = Prefix::Nickname(
                    sender_name.to_owned(),
                    sender_id.to_string(),
                    "villv.tech".to_owned(),
                );
                content
                    .into_iter()
                    .map(|content| {
                        prefixed(
                            nick_prefix.clone(),
                            Message::from(Command::PRIVMSG(format!("#{group_id}"), content)),
                        )
                    })
                    .collect()
            }
            RenderedOnebotMessage::Private {
                content,
                sender_id,
                sender_name,
            } => {
                let nick_prefix = Prefix::Nickname(
                    sender_name.to_owned(),
                    sender_id.to_string(),
                    "villv.tech".to_owned(),
                );
                content
                    .into_iter()
                    .map(|content| {
                        prefixed(
                            nick_prefix.clone(),
                            Message::from(Command::PRIVMSG("rivus".to_owned(), content)),
                        )
                    })
                    .collect()
            }
        };

        for msg in messages {
            irc_tx
                .lock()
                .await
                .feed(msg)
                .await
                .context("irc connection broken")?;
        }
        irc_tx
            .lock()
            .await
            .flush()
            .await
            .context("irc connection broken")?;
    }
}

async fn handle_irc_connection(
    conn: tokio::net::TcpStream,
    onebot_rx: tokio::sync::broadcast::Receiver<RenderedOnebotMessage>,
) {
    let codec = irc_proto::IrcCodec::new("irc_codec").unwrap();
    let irc_conn = codec.framed(conn);
    let (irc_tx, irc_rx) = irc_conn.split();
    let bot = loop {
        if let Some(bot) = kovi_broadcast_plugin::RUNTIME_BOT.get() {
            break bot.clone();
        }
        tokio::task::yield_now().await;
    };

    let irc_tx = Arc::new(Mutex::new(irc_tx));
    let shutdown_token = CancellationToken::new();
    let _shutdown_after_return = shutdown_token.drop_guard_ref();
    tokio::select! {
        _ = tokio::spawn(handle_irc_messages(
                irc_rx,
                irc_tx.clone(),
                bot,
                shutdown_token.clone(),
        )) => (),
        _ = tokio::spawn(handle_onebot_messages(
                onebot_rx,
                irc_tx.clone(),
                shutdown_token.clone(),
        )) => (),
    };
}

async fn irc_server_main(
    bind_addr: std::net::SocketAddr,
    broadcast_tx: tokio::sync::broadcast::Sender<RenderedOnebotMessage>,
) {
    let acceptor = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    while let Ok((conn, peer)) = acceptor.accept().await {
        log::info!("incoming connection: {peer}");
        handle_irc_connection(conn, broadcast_tx.subscribe()).await;
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
