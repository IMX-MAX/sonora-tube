//! Prints a collab video's makers and a channel's parts, to check the parsers against live
//! YouTube: `cargo run -p tube --example parts -- <video> <channel>`.

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let mut asked = std::env::args().skip(1);
    let video = asked.next().unwrap_or_else(|| "5bevi_0v6wU".to_owned());
    let channel = asked
        .next()
        .unwrap_or_else(|| "UCqECaJ8Gagnn7YCbPEzWH6g".to_owned());
    let client = tube::Client::new();
    let watch = client.watch(&video).await?;
    println!("channel: {:?}", watch.channel);
    println!("partners: {:?}", watch.partners);
    println!(
        "top: {} newest: {}",
        watch.top.is_some(),
        watch.newest.is_some()
    );
    if let Some(token) = &watch.newest {
        let comments = client.comments(token).await?;
        println!(
            "newest: {:?}",
            comments.page.items.first().map(|c| &c.published)
        );
    }
    let page = client.channel(&channel).await?;
    println!("tabs: {:?}", page.tabs);
    for tab in page.tabs.iter().filter(|tab| **tab != tube::Tab::Videos) {
        let section = client.channel_section(&channel, *tab).await?;
        println!(
            "{tab:?}: {} playlists (more {}), {} posts",
            section.playlists.items.len(),
            section.playlists.more.is_some(),
            section.posts.items.len()
        );
        println!("  {:?}", section.playlists.items.first());
        if let Some(post) = section.posts.items.first() {
            println!(
                "  {:?} {:?} {} pictures, video {:?}, {} chars",
                post.published,
                post.likes,
                post.pictures.len(),
                post.video.as_ref().map(|video| &video.id),
                post.text.len()
            );
        }
    }
    Ok(())
}
