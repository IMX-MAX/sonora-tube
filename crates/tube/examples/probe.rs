//! Prints what the client reads, to check the parsers against live YouTube:
//! `cargo run -p tube --example probe -- <query>`.

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "big buck bunny".to_owned());
    let client = tube::Client::new();
    let found = client.search(&query).await?;
    println!(
        "search: {} items, more: {}",
        found.items.len(),
        found.more.is_some()
    );
    for item in found.items.iter().take(6) {
        println!("  {item:?}");
    }
    let Some(video) = found.items.iter().find_map(|item| match item {
        tube::Found::Video(video) => Some(video.clone()),
        _ => None,
    }) else {
        return Ok(());
    };
    let watch = client.watch(&video.id).await?;
    println!(
        "watch: {} | {:?} | {:?} | likes {:?} | {:?} | related {} | description {} chars",
        watch.title,
        watch.views,
        watch.published,
        watch.likes,
        watch.channel,
        watch.related.len(),
        watch.description.len()
    );
    println!("  {:?}", watch.related.first());
    if let Some(token) = &watch.comments {
        let comments = client.comments(token).await?;
        println!(
            "comments: {:?}, {} on the page, more: {}",
            comments.total,
            comments.page.items.len(),
            comments.page.more.is_some()
        );
        println!("  {:?}", comments.page.items.first());
        if let Some(replies) = comments.page.items.iter().find_map(|c| c.replies.clone()) {
            let replies = client.comments(&replies).await?;
            println!(
                "replies: {} | {:?}",
                replies.page.items.len(),
                replies.page.items.first()
            );
        }
    }
    if let Some(channel) = &watch.channel {
        let page = client.channel(&channel.id).await?;
        println!(
            "channel: {:?} | {} videos, more: {}",
            page.channel,
            page.videos.items.len(),
            page.videos.more.is_some()
        );
        println!("  {:?}", page.videos.items.first());
        if let Some(token) = &page.videos.more {
            println!("  next: {}", client.videos_more(token).await?.items.len());
        }
    }
    let list = client
        .playlist("PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI")
        .await?;
    println!(
        "playlist: {} | {} | more: {}",
        list.title,
        list.videos.items.len(),
        list.videos.more.is_some()
    );
    println!("  {:?}", list.videos.items.first());
    Ok(())
}
