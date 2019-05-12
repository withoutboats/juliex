#![feature(async_await)]

use std::io;

use futures::StreamExt;
use futures::executor;
use futures::io::AsyncReadExt;

use romio::{TcpListener, TcpStream};

fn main() -> io::Result<()> {
    executor::block_on(async {
        let mut listener = TcpListener::bind(&"127.0.0.1:7878".parse().unwrap())?;
        let mut incoming = listener.incoming();

        println!("Listening on 127.0.0.1:7878");

        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            let addr = stream.peer_addr()?;

            juliex::spawn(async move {
                println!("Accepting stream from: {}", addr);

                echo_on(stream).await.unwrap();

                println!("Closing stream from: {}", addr);
            });
        }

        Ok(())
    })
}

async fn echo_on(stream: TcpStream) -> io::Result<()> {
    let (mut reader, mut writer) = stream.split();
    reader.copy_into(&mut writer).await?;
    Ok(())
}
