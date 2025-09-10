========
如何使用
========

1) 运行 `cargo run --release`，程序会询问 onebot 服务的相关信息。**目前只支持正向 websocket 协议[1]！**
2) irc 服务器会监听 0.0.0.0:8621 端口。硬编码的，后面再改
3) 把 irssi[2] 的 **NICKNAME** 改成 `rivus` （其它名字可能工作也可能不工作，不知道）
4) 开始聊天

[1]: https://www.napcat.wiki/onebot/network#_2-2-napcatqq-%E4%BD%9C%E4%B8%BA-websocket-%E6%9C%8D%E5%8A%A1%E5%99%A8%E6%8E%A5%E6%94%B6%E4%BA%8B%E4%BB%B6%E5%92%8C%E8%AF%B7%E6%B1%82
[2]: 其它客户端好像不工作，hexchat 和 thunderbird 都不行，不知道为啥，如果有懂的希望能帮我修修……
