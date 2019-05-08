#![feature(async_await)]

use std::io;

use futures::StreamExt;
use futures::executor;
use futures::io::AsyncReadExt;
use futures::io::AsyncWriteExt;

use romio::{TcpListener};

fn main() -> io::Result<()> {
    executor::block_on(async {
        let mut listener = TcpListener::bind(&"127.0.0.1:7878".parse().unwrap())?;
        let mut incoming = listener.incoming();

        println!("Listening on 127.0.0.1:7878");

        while let Some(stream) = incoming.next().await {
            let stream = stream?;

            juliex::spawn(async move {
              let (_, mut writer) = stream.split();
              writer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length:0\r\n\r\n").await.unwrap();
            });
        }

        Ok(())
    })
}
