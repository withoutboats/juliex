# juliex - a minimal futures executor

__Note: This library is not actively being developed. If you're looking for an
`async/await` compatible runtime consider using
[`async-std`](https://github.com/async-rs/async-std).__

juliex is a concurrent executor for Rust futures. It is implemented as a
threadpool executor using a single, shared queue. Algorithmically, it is very
similar to the Threadpool executor provided by the futures crate. The main
difference is that juliex uses a crossbeam channel and performs a single
allocation per spawned future, whereas the futures Threadpool uses std
concurrency primitives and multiple allocations.

Similar to [romio][romio] - an IO reactor - juliex currently provides no user
configuration. It exposes the most minimal API possible.

## Example
```rust
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
```

## Safety
This crate uses `unsafe` internally around atomic access. Invariants around this
are manually checked.

## License
MIT OR Apache-2.0

[romio]: https://github.com/withoutboats/romio
