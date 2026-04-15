use std::{
    fs::{self, File, OpenOptions}, io::{Error, ErrorKind, Write}, path::Path
};

use log::{debug};
use serde::{Deserialize, Serialize};
use url::Url;
use chrono::prelude::*;

use reqwest::{blocking::multipart::{ Form, Part }};
use crate::{ Client, JfResult };

#[derive(Deserialize, Serialize, Clone)]
struct ImageUrl {
    id: String,
    url: String,
    timestamp: String
}

impl ImageUrl {
    fn new<T: Into<String>, Y: Into<String>, U: Into<String>>(id: T, url: Y, timestamp: U) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            timestamp: timestamp.into()
        }
    }
}

pub fn get_image(client: &Client) -> JfResult<Url> {
    let mut image_urls = read_file(client)?;

    if let Some(idx) = image_urls
        .iter()
        .position(|image_url| client.session.as_ref().unwrap().item_id == image_url.id)
    {
        let image_url = image_urls[idx].clone();

        debug!("Found image url: \"{}\"", image_url.url.clone());

        let file_timestamp = image_url.timestamp.parse::<i64>().unwrap();
        let file_date = DateTime::from_timestamp(file_timestamp, 0).unwrap();
        let now = Utc::now();

        let diff = now - file_date;

        debug!("Image is {} hours old.", diff.num_hours().to_string());

        if diff.num_hours() >= 72 {
            debug!("Image \"{}\" is expired", image_url.url.clone());

            image_urls.swap_remove(idx);
            
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&client.litterbox_options.urls_location)?;

            file.write_all(serde_json::to_string(&image_urls)?.as_bytes())?;

            let _ = file.flush();

            // Try again
            self::get_image(client)
        }
        else {
            Ok(Url::parse(&image_url.url)?)
        }
    } else {
        debug!("No cached litterbox image found. Uploading new image");

        let current_time: DateTime<Utc> = Utc::now();

        let litterbox_url = upload(client)?;

        debug!("Litterbox response: {}", litterbox_url);

        let image_url = ImageUrl::new(
            &client.session.as_ref().unwrap().item_id,
            litterbox_url.as_str(),
            current_time.timestamp().to_string()
        );

        image_urls.push(image_url);

        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&client.litterbox_options.urls_location)?;

        file.write_all(serde_json::to_string(&image_urls)?.as_bytes())?;

        let _ = file.flush();

        Ok(litterbox_url)
    }
}

fn read_file(client: &Client) -> JfResult<Vec<ImageUrl>> {
    if let Ok(contents_raw) = fs::read_to_string(&client.litterbox_options.urls_location) {
        if let Ok(contents) = serde_json::from_str::<Vec<ImageUrl>>(&contents_raw) {
            return Ok(contents);
        }
    }

    let path = Path::new(&client.litterbox_options.urls_location)
        .parent()
        .ok_or(Error::new(
            ErrorKind::Other,
            "Can't find parent folder of urls.json",
        ))?;

    fs::create_dir_all(path)?;

    let mut file = File::create(client.litterbox_options.urls_location.clone())?;

    let new: Vec<ImageUrl> = vec![];

    file.write_all(serde_json::to_string(&new)?.as_bytes())?;

    let _ = file.flush();

    Ok(new)
}

fn upload(client: &Client) -> JfResult<Url> {
    let image_bytes = client.reqwest.get(client.get_image()?).send()?.bytes()?;

    debug!("Uploading image to litterbox");

    let litterbox_client = reqwest::blocking::Client::builder().build()?;
    let filename = Utc::now().to_string();

    let file_bytes = if client.process_images {
        use crate::external::image_utils::make_square_with_blur;
        make_square_with_blur(&image_bytes, &client.image_processing_options)?
    } else {
        image_bytes.to_vec()
    };

    let litterbox_form = Form::new()
        .text("reqtype", "fileupload")
        .text("time", "72h")
        .part("fileToUpload", Part::bytes(file_bytes).file_name(filename + ".jpg"));

    let res: String = litterbox_client
        .post("https://litterbox.catbox.moe/resources/internals/api.php")
        .multipart(litterbox_form)
        .send()?
        .text()?;

    debug!("Response from Litterbox: \"{}\"", res.clone());

    Ok(Url::parse(&res)?)
}