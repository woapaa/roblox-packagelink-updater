use anyhow::Result;
use flate2::read::GzDecoder;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use rbx_dom_weak::{WeakDom, ustr};
use rbx_types::{Ref, Variant};
use reqwest::{Response, Url, cookie::Jar};
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use rustyline::DefaultEditor;
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    path::Path,
    sync::Arc,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

mod asset_response;
use asset_response::AssetResponse;

mod universe_places_response;
use universe_places_response::UniversePlacesResponse;

mod roblox_cookie;
use roblox_cookie::get_roblosecurity;

async fn decompress_if_needed(binary_response: Response) -> Result<Vec<u8>> {
    // weird bug reqwest wouldn't decompress it so i had to add this
    let is_gzipped = binary_response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .map_or(false, |val| val == "gzip");

    let body_bytes = binary_response.bytes().await?;
    let mut decompressed_bytes = Vec::new();

    if is_gzipped {
        let mut decoder = GzDecoder::new(&body_bytes[..]);
        decoder.read_to_end(&mut decompressed_bytes)?;
    } else {
        decompressed_bytes = body_bytes.to_vec();
    }

    return Ok(decompressed_bytes);
}

struct ToWork {
    package_id_numbers: String,
    package_link: Ref,
    package_link_group: Ref,
    package_link_parent: Ref,
}

struct PlaceData {
    id: u64,
    name: String,
    dom: WeakDom,
    to_work: Vec<ToWork>,
}

struct SavedPlace {
    id: u64,
    name: String,
    buffer: Vec<u8>,
}

async fn collect_places_and_package_ids(
    client: Arc<reqwest_middleware::ClientWithMiddleware>,
    universe_id: u64,
    spinner_style: ProgressStyle,
    failed_tx: UnboundedSender<String>,
) -> Result<Vec<PlaceData>> {
    let universe_fetch_pb = ProgressBar::new(1);
    universe_fetch_pb.set_style(spinner_style.clone());
    universe_fetch_pb.set_prefix("[universe]");
    universe_fetch_pb.set_message("Fetching places list");

    let response = client
        .get(format!(
            "https://develop.roblox.com/v1/universes/{universe_id}/places?sortOrder=Asc&limit=100"
        ))
        .send()
        .await?
        .json::<UniversePlacesResponse>()
        .await?;

    universe_fetch_pb.finish_and_clear();

    println!(
        "
Found places:"
    );
    for place in response.data() {
        println!("> {} (id: {})", place.name(), place.id());
    }

    // Download each place once, parse and record PackageLink occurrences
    let places_pb = ProgressBar::new(response.data().len() as u64);
    places_pb.set_style(spinner_style.clone());
    places_pb.set_prefix("[places]");

    let mut places_data: Vec<PlaceData> = Vec::new();

    for place in response.data().iter().cloned() {
        places_pb.set_message(format!(
            "Downloading place {} ({})",
            place.name(),
            place.id()
        ));
        // Fetch place asset metadata
        let place_asset_resp = client
            .get(format!(
                "https://assetdelivery.roblox.com/v2/asset/?id={}",
                place.id()
            ))
            .send()
            .await;

        let place_asset_resp = match place_asset_resp {
            Ok(r) => r,
            Err(e) => {
                let msg = format!(
                    "Failed to fetch asset metadata for place {} {}: {}",
                    place.name(),
                    place.id(),
                    e
                );
                let _ = failed_tx.send(msg);
                places_pb.inc(1);
                continue;
            }
        };

        let place_asset_json = match place_asset_resp.json::<AssetResponse>().await {
            Ok(j) => j,
            Err(e) => {
                let msg = format!(
                    "Failed to parse asset metadata for place {} {}: {}",
                    place.name(),
                    place.id(),
                    e
                );
                let _ = failed_tx.send(msg);
                places_pb.inc(1);
                continue;
            }
        };

        // Find CDN source
        let mut cdn = None;
        for location in place_asset_json.locations() {
            if location.asset_format() == "source" {
                cdn = Some(location.location());
                break;
            }
        }

        if cdn.is_none() {
            let msg = format!(
                "CDN source not found for place {} {}",
                place.name(),
                place.id()
            );
            let _ = failed_tx.send(msg);
            places_pb.inc(1);
            continue;
        }

        let cdn = cdn.unwrap();
        places_pb.set_message(format!("Fetching CDN for place {}: {}", place.id(), cdn));
        let place_binary_response = match client.get(cdn).send().await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!(
                    "Failed to GET place CDN {} for {} {}: {}",
                    cdn,
                    place.name(),
                    place.id(),
                    e
                );
                let _ = failed_tx.send(msg);
                places_pb.inc(1);
                continue;
            }
        };

        let place_bytes = match decompress_if_needed(place_binary_response).await {
            Ok(b) => b,
            Err(e) => {
                let msg = format!(
                    "Failed to decompress place {} {}: {}",
                    place.name(),
                    place.id(),
                    e
                );
                let _ = failed_tx.send(msg);
                places_pb.inc(1);
                continue;
            }
        };

        places_pb.set_message(format!("Parsing place DOM {}", place.id()));
        let reader = Cursor::new(place_bytes);
        let dom = match rbx_binary::from_reader(reader) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!(
                    "Failed to parse RBX binary for place {} {}: {}",
                    place.name(),
                    place.id(),
                    e
                );
                let _ = failed_tx.send(msg);
                places_pb.inc(1);
                continue;
            }
        };

        // Scan for PackageLink instances
        let mut to_work: Vec<ToWork> = Vec::new();
        for instance in dom.descendants() {
            if instance.class == "PackageLink" {
                // Get PackageId
                let package_id = match instance.properties.get(&ustr("PackageId")) {
                    Some(Variant::ContentId(id)) => id.clone(),
                    _ => {
                        let msg = format!(
                            "PackageLink without valid PackageId in place {} {}",
                            place.name(),
                            place.id()
                        );
                        let _ = failed_tx.send(msg);
                        continue;
                    }
                };

                let package_id_numbers = match package_id.as_str().strip_prefix("rbxassetid://") {
                    Some(s) => s.to_string(),
                    None => {
                        let msg = format!(
                            "PackageId had unexpected format '{}' in place {} {}",
                            package_id.as_str(),
                            place.name(),
                            place.id()
                        );
                        let _ = failed_tx.send(msg);
                        continue;
                    }
                };

                let package_link_group = instance.parent();
                let package_link = instance.referent();
                let package_link_parent = dom.get_by_ref(package_link_group).unwrap().parent();

                to_work.push(ToWork {
                    package_id_numbers,
                    package_link,
                    package_link_group,
                    package_link_parent,
                });
            }
        }

        places_data.push(PlaceData {
            id: *place.id(),
            name: place.name().to_string(),
            dom,
            to_work,
        });

        places_pb.inc(1);
    }

    places_pb.finish_with_message("Finished scanning places");

    Ok(places_data)
}

async fn fetch_package_assets(
    client: Arc<reqwest_middleware::ClientWithMiddleware>,
    package_ids: Vec<String>,
    spinner_style: ProgressStyle,
    failed_tx: UnboundedSender<String>,
) -> HashMap<String, Vec<u8>> {
    let packages_pb = ProgressBar::new(package_ids.len() as u64);
    packages_pb.set_style(spinner_style.clone());
    packages_pb.set_prefix("[packages]");

    let package_results =
        futures::stream::iter(package_ids.into_iter().map(|package_id_numbers| {
            let client = Arc::clone(&client);
            let packages_pb = packages_pb.clone();
            let failed_tx = failed_tx.clone();
            async move {
                packages_pb.set_message(format!("Finding CDN for package {}", package_id_numbers));

                // Get asset metadata
                let asset_meta = match client
                    .get(format!(
                        "https://assetdelivery.roblox.com/v2/asset/?id={}",
                        package_id_numbers
                    ))
                    .send()
                    .await
                {
                    Ok(r) => match r.json::<AssetResponse>().await {
                        Ok(j) => j,
                        Err(e) => {
                            let msg = format!(
                                "Failed parse package asset metadata {}: {}",
                                package_id_numbers, e
                            );
                            let _ = failed_tx.send(msg);
                            packages_pb.inc(1);
                            return Err((package_id_numbers, "parse_meta_failed".to_string()));
                        }
                    },
                    Err(e) => {
                        let msg = format!(
                            "Failed GET package asset metadata {}: {}",
                            package_id_numbers, e
                        );
                        let _ = failed_tx.send(msg);
                        packages_pb.inc(1);
                        return Err((package_id_numbers, "meta_failed".to_string()));
                    }
                };

                let mut cdn = None;
                for location in asset_meta.locations() {
                    if location.asset_format() == "source" {
                        cdn = Some(location.location());
                        break;
                    }
                }

                if cdn.is_none() {
                    let msg = format!("Failed to find CDN for package {}", package_id_numbers);
                    let _ = failed_tx.send(msg);
                    packages_pb.inc(1);
                    return Err((package_id_numbers, "cdn_not_found".to_string()));
                }

                let cdn = cdn.unwrap();
                packages_pb.set_message(format!(
                    "Downloading package {} from CDN",
                    package_id_numbers
                ));
                let package_binary_response = match client.get(cdn).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!(
                            "Failed GET package CDN {} for {}: {}",
                            cdn, package_id_numbers, e
                        );
                        let _ = failed_tx.send(msg);
                        packages_pb.inc(1);
                        return Err((package_id_numbers, "cdn_get_failed".to_string()));
                    }
                };

                let package_bytes = match decompress_if_needed(package_binary_response).await {
                    Ok(b) => b,
                    Err(e) => {
                        let msg =
                            format!("Failed decompress package {}: {}", package_id_numbers, e);
                        let _ = failed_tx.send(msg);
                        packages_pb.inc(1);
                        return Err((package_id_numbers, "decompress_failed".to_string()));
                    }
                };

                packages_pb.inc(1);
                Ok((package_id_numbers, package_bytes))
            }
        }))
        .buffer_unordered(3)
        .collect::<Vec<Result<(String, Vec<u8>), (String, String)>>>()
        .await;

    packages_pb.finish_with_message("Finished fetching packages");

    // Collect successful package bytes
    let mut package_bytes_map: HashMap<String, Vec<u8>> = HashMap::new();
    for res in package_results.into_iter() {
        match res {
            Ok((id, bytes)) => {
                package_bytes_map.insert(id, bytes);
            }
            Err((id, _)) => {
                let msg = format!(
                    "Package {} failed to fetch (see earlier messages). Leaving PackageLink(s) untouched.",
                    id
                );
                let _ = failed_tx.send(msg);
            }
        }
    }

    package_bytes_map
}

async fn process_places_and_save(
    places_data: Vec<PlaceData>,
    package_bytes_map: HashMap<String, Vec<u8>>,
    spinner_style: ProgressStyle,
    failed_tx: UnboundedSender<String>,
) -> Result<Vec<SavedPlace>> {
    let save_pb = ProgressBar::new(places_data.len() as u64);
    save_pb.set_style(spinner_style.clone());
    save_pb.set_prefix("[save]");

    let mut saved_places: Vec<SavedPlace> = Vec::new();

    for mut place in places_data.into_iter() {
        save_pb.set_message(format!(
            "Processing replacements for place {} ({})",
            place.name, place.id
        ));
        let mut replacements = 0u32;
        for work in place.to_work.iter() {
            if let Some(bytes) = package_bytes_map.get(&work.package_id_numbers) {
                let package_reader = Cursor::new(bytes.clone());
                let mut package_dom = match rbx_binary::from_reader(package_reader) {
                    Ok(d) => d,
                    Err(e) => {
                        let msg = format!(
                            "Failed to parse package DOM for package {}: {}",
                            work.package_id_numbers, e
                        );
                        let _ = failed_tx.send(msg);
                        continue;
                    }
                };

                let package_root = package_dom.root().children()[0];

                // Transfer the old PackageLink into package_dom
                place
                    .dom
                    .transfer(work.package_link, &mut package_dom, package_root);

                // Destroy the old package
                place.dom.destroy(work.package_link_group);

                // Transfer package contents into the place DOM under the same parent
                package_dom.transfer(package_root, &mut place.dom, work.package_link_parent);

                replacements += 1;
            } else {
                let msg = format!(
                    "No fetched asset for package {} referenced in place {} {} - leaving untouched.",
                    work.package_id_numbers, place.name, place.id
                );
                let _ = failed_tx.send(msg);
                continue;
            }
        }

        save_pb.set_message(format!(
            "Serializing place {} ({}) with {} replacements",
            place.name, place.id, replacements
        ));
        let mut buffer = Vec::new();
        rbx_binary::to_writer(&mut buffer, &place.dom, place.dom.root().children())?;

        save_pb.set_message(format!("Saving to /rbxls/{}.rbxl", place.id));
        let folder = Path::new("rbxls");
        tokio::fs::create_dir_all(folder).await?;
        let file_path = folder.join(format!("{}.rbxl", place.id));
        tokio::fs::write(&file_path, &buffer).await?;

        saved_places.push(SavedPlace {
            id: place.id,
            name: place.name,
            buffer,
        });

        save_pb.inc(1);
    }

    save_pb.finish_with_message("Saved all updated places locally (not published)");

    Ok(saved_places)
}

async fn publish_saved_places(
    saved_places: Vec<SavedPlace>,
    client: Arc<reqwest_middleware::ClientWithMiddleware>,
    rbxl_api_key: String,
    universe_id: u64,
    spinner_style: ProgressStyle,
    failed_tx: UnboundedSender<String>,
) {
    let publish_pb = ProgressBar::new(saved_places.len() as u64);
    publish_pb.set_style(spinner_style.clone());
    publish_pb.set_prefix("[publish]");

    let publish_results = futures::stream::iter(saved_places.into_iter().map(|saved| {
        let client = Arc::clone(&client);
        let rbxl_api_key = rbxl_api_key.clone();
        let publish_pb = publish_pb.clone();
        let failed_tx = failed_tx.clone();
        let universe_id = universe_id;
        async move {
            publish_pb.set_message(format!("Publishing place {} ({})", saved.name, saved.id));
            let publish_response = client
                .post(format!("https://apis.roblox.com/universes/v1/{}/places/{}/versions?versionType=Published", universe_id, saved.id))
                .header("x-api-key", rbxl_api_key)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", saved.buffer.len())
                .body(saved.buffer)
                .send()
                .await;

            match publish_response {
                Ok(r) => {
                    if r.status().is_success() {
                        publish_pb.inc(1);
                        Ok(saved.id)
                    } else {
                        let msg = format!("Failed to publish place {} {}: HTTP {}", saved.name, saved.id, r.status());
                        let _ = failed_tx.send(msg);
                        publish_pb.inc(1);
                        Err(saved.id)
                    }
                }
                Err(e) => {
                    let msg = format!("Failed to publish place {} {}: {}", saved.name, saved.id, e);
                    let _ = failed_tx.send(msg);
                    publish_pb.inc(1);
                    Err(saved.id)
                }
            }
        }
    }))
    .buffer_unordered(3)
    .collect::<Vec<Result<u64, u64>>>()
    .await;

    publish_pb.finish_and_clear();

    let total = publish_results.len();
    let succeeded = publish_results.iter().filter(|r| r.is_ok()).count();
    let failed = total - succeeded;
    println!(
        "Publishing complete: {} succeeded, {} failed (out of {})",
        succeeded, failed, total
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    // Read environment variables from .env
    dotenv::dotenv().ok();

    // Set up rustyline
    let mut rl = DefaultEditor::new()?;

    let mut rbxl_api_key: String = dotenv::var("RBXL_API_KEY").unwrap_or("".to_string());
    let mut rbxl_cookie: String = dotenv::var("RBXL_COOKIE").unwrap_or("".to_string());

    if rbxl_api_key.is_empty() {
        rbxl_api_key = rl.readline(
            ":: Input Roblox API Key
>> ",
        )?;
    }
    if rbxl_cookie.is_empty() {
        let auto_find_cookie_confirm = rl
            .readline(
                "
:: There is no set .ROBLOSECURITY, would you like to automatically try
:: find it? (yes/no)
>> ",
            )?
            .to_lowercase()
            == "yes";
        if auto_find_cookie_confirm {
            rbxl_cookie = get_roblosecurity()?;
            println!(":: Successfully retrieved .ROBLOSECURITY\n");
        } else {
            rbxl_cookie = rl.readline(
                ":: Input Roblox .ROBLOSECURITY
>> ",
            )?;
        }
    }

    // Progress style
    let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
        .unwrap()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

    // Set up a client with exponential backoff
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let jar = Jar::default();
    jar.add_cookie_str(
        &format!(".ROBLOSECURITY={rbxl_cookie}"),
        &"https://assetdelivery.roblox.com".parse::<Url>().unwrap(),
    );
    let cookies = Arc::new(jar);
    let client = ClientBuilder::new(
        reqwest::Client::builder()
            .cookie_provider(Arc::clone(&cookies))
            .timeout(std::time::Duration::from_secs(20))
            .build()?,
    )
    .with(RetryTransientMiddleware::new_with_policy(retry_policy))
    .build();

    // Prompt for UniverseId
    let mut universe_id: String = dotenv::var("RBXL_UNIVERSE_ID").unwrap_or("".to_string());
    if universe_id.is_empty() {
        universe_id = rl.readline(
            ":: Input Universe Id
>> ",
        )?;
    }
    let universe_id = universe_id.trim().parse()?;
    let client = Arc::new(client);

    // Failure collector
    let (failed_tx, mut failed_rx): (UnboundedSender<String>, UnboundedReceiver<String>) =
        tokio::sync::mpsc::unbounded_channel();

    // Collect places and package ids
    let places_data = collect_places_and_package_ids(
        Arc::clone(&client),
        universe_id,
        spinner_style.clone(),
        failed_tx.clone(),
    )
    .await?;

    // Build unique package set
    let mut unique_packages: HashSet<String> = HashSet::new();
    for p in &places_data {
        for w in &p.to_work {
            unique_packages.insert(w.package_id_numbers.clone());
        }
    }

    println!(
        "Found {} unique package ids to fetch",
        unique_packages.len()
    );

    // Fetch package assets
    let packages_vec: Vec<String> = unique_packages.into_iter().collect();
    let package_bytes_map = fetch_package_assets(
        Arc::clone(&client),
        packages_vec,
        spinner_style.clone(),
        failed_tx.clone(),
    )
    .await;

    // Process places and save locally
    let saved_places = process_places_and_save(
        places_data,
        package_bytes_map,
        spinner_style.clone(),
        failed_tx.clone(),
    )
    .await?;

    // Drain any immediate failures so far. We'll collect all later too.
    let mut early_failures: Vec<String> = Vec::new();
    while let Ok(msg) = failed_rx.try_recv() {
        early_failures.push(msg);
    }

    if !early_failures.is_empty() {
        println!(
            "
Failures / warnings encountered during scanning/fetching/replacement:"
        );
        for s in early_failures.iter() {
            println!("- {}", s);
        }
    }

    // Now wait for user permission to publish all saved places
    let publish_confirm = rl
        .readline(
            "
:: Publish all saved places now? (yes/no)
>> ",
        )?
        .to_lowercase()
        == "yes";
    if !publish_confirm {
        println!("Publishing skipped. Local files are available under ./rbxls/*.rbxl");

        // Drain remaining messages so user can inspect them
        drop(failed_tx);
        let mut remaining: Vec<String> = Vec::new();
        while let Some(msg) = failed_rx.recv().await {
            remaining.push(msg);
        }

        if !remaining.is_empty() {
            println!(
                "
Additional failures captured:"
            );
            for s in remaining.iter() {
                println!("- {}", s);
            }
        }

        rl.readline(
            ":: Press enter to exit
>> ",
        )?;
        return Ok(());
    }

    // Publish
    publish_saved_places(
        saved_places,
        Arc::clone(&client),
        rbxl_api_key,
        universe_id,
        spinner_style.clone(),
        failed_tx.clone(),
    )
    .await;

    // After publishing, collect all failure messages from channel and display it if there are any
    drop(failed_tx);
    let mut failures: Vec<String> = Vec::new();
    while let Some(msg) = failed_rx.recv().await {
        failures.push(msg);
    }

    if !failures.is_empty() {
        println!(
            "
Failures / warnings encountered during operation:"
        );
        for s in failures.iter() {
            println!("- {}", s);
        }
    } else {
        println!(
            "
All operations completed successfully."
        );
    }

    rl.readline(
        ":: Press enter to exit
>> ",
    )?;

    Ok(())
}
