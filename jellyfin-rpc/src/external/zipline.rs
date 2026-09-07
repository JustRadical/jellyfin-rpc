use std::{
    fs::{self, File, OpenOptions},
    io::{Error, ErrorKind, Write},
    path::Path,
};

use chrono::{DateTime, Duration, Utc};
use log::debug;
use reqwest::blocking::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Client, JfResult};

#[derive(Deserialize, Serialize, Clone)]
struct ImageUrl {
    id: String,
    url: String,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct UploadResponse {
    files: Vec<UploadedFile>,
    #[serde(rename = "deletesAt")]
    deletes_at: Option<String>,
}

#[derive(Deserialize)]
struct UploadedFile {
    url: String,
}

pub fn get_image(client: &Client) -> JfResult<Url> {
    let mut image_urls = read_file(client)?;
    let item_id = &client.session.as_ref().unwrap().item_id;

    if let Some(idx) = image_urls
        .iter()
        .position(|image_url| item_id == &image_url.id)
    {
        let image_url = image_urls[idx].clone();

        if image_url.expires_at.as_deref().is_some_and(|expires_at| {
            DateTime::parse_from_rfc3339(expires_at)
                .map(|expires_at| expires_at <= Utc::now())
                .unwrap_or(true)
        }) {
            debug!("Zipline image \"{}\" is expired", image_url.url);
            image_urls.swap_remove(idx);
            write_file(client, &image_urls)?;
        } else {
            debug!("Found Zipline image URL: \"{}\"", image_url.url);
            return Ok(Url::parse(&image_url.url)?);
        }
    }

    debug!("No cached Zipline image found. Uploading new image");
    let (url, expires_at) = upload(client)?;
    image_urls.push(ImageUrl {
        id: item_id.clone(),
        url: url.to_string(),
        expires_at,
    });
    write_file(client, &image_urls)?;

    Ok(url)
}

fn read_file(client: &Client) -> JfResult<Vec<ImageUrl>> {
    if let Ok(contents_raw) = fs::read_to_string(&client.zipline_options.urls_location) {
        if let Ok(contents) = serde_json::from_str::<Vec<ImageUrl>>(&contents_raw) {
            return Ok(contents);
        }
    }

    let path = Path::new(&client.zipline_options.urls_location)
        .parent()
        .ok_or(Error::other(
            "Can't find parent folder of Zipline urls.json",
        ))?;

    fs::create_dir_all(path)?;
    let image_urls = Vec::new();
    let mut file = File::create(&client.zipline_options.urls_location)?;
    file.write_all(serde_json::to_string(&image_urls)?.as_bytes())?;
    let _ = file.flush();

    Ok(image_urls)
}

fn write_file(client: &Client, image_urls: &[ImageUrl]) -> JfResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&client.zipline_options.urls_location)?;
    file.write_all(serde_json::to_string(image_urls)?.as_bytes())?;
    let _ = file.flush();
    Ok(())
}

fn upload(client: &Client) -> JfResult<(Url, Option<String>)> {
    let image_bytes = client.reqwest.get(client.get_image()?).send()?.bytes()?;
    let timestamp = Utc::now().timestamp();
    let (file_bytes, filename, mime) = if client.process_images {
        use crate::external::image_utils::make_square_with_blur;
        (
            make_square_with_blur(&image_bytes, &client.image_processing_options)?,
            format!("{}.png", timestamp),
            "image/png",
        )
    } else {
        (
            image_bytes.to_vec(),
            format!("{}.jpg", timestamp),
            "image/jpeg",
        )
    };

    let part = Part::bytes(file_bytes).file_name(filename).mime_str(mime)?;
    let form = Form::new().part("file", part);
    let options = &client.zipline_options;
    let mut request = client
        .reqwest
        .post(&options.url)
        .header("authorization", &options.token)
        .multipart(form);

    for (name, value) in [
        ("x-zipline-deletes-at", options.expiry.as_ref()),
        ("x-zipline-format", options.format.as_ref()),
        (
            "x-zipline-image-compression-type",
            options.image_compression_type.as_ref(),
        ),
        ("x-zipline-folder", options.folder.as_ref()),
        ("x-zipline-domain", options.domain.as_ref()),
    ] {
        if let Some(value) = value {
            request = request.header(name, value);
        }
    }

    if let Some(percent) = options.image_compression_percent {
        request = request.header("x-zipline-image-compression-percent", percent.to_string());
    }
    if options.original_name {
        request = request.header("x-zipline-original-name", "true");
    }

    debug!("Uploading image to Zipline");
    let response = request
        .send()?
        .error_for_status()?
        .json::<UploadResponse>()?;
    let file = response.files.into_iter().next().ok_or(Error::new(
        ErrorKind::InvalidData,
        "Zipline upload response did not contain a file",
    ))?;
    let expires_at = response.deletes_at.or_else(|| {
        options
            .expiry
            .as_deref()
            .and_then(expiry_from_header)
            .map(|expires_at| expires_at.to_rfc3339())
    });

    Ok((Url::parse(&file.url)?, expires_at))
}

fn expiry_from_header(value: &str) -> Option<DateTime<Utc>> {
    if let Some(date) = value.strip_prefix("date=") {
        return DateTime::parse_from_rfc3339(date)
            .ok()
            .map(|date| date.with_timezone(&Utc));
    }

    let unit = value.chars().last()?;
    let amount = value[..value.len() - unit.len_utf8()].parse::<i64>().ok()?;
    let duration = match unit {
        's' => Duration::seconds(amount),
        'm' => Duration::minutes(amount),
        'h' => Duration::hours(amount),
        'd' => Duration::days(amount),
        'w' => Duration::weeks(amount),
        _ => return None,
    };
    Utc::now().checked_add_signed(duration)
}

#[cfg(test)]
mod tests {
    use super::expiry_from_header;

    #[test]
    fn parses_relative_expiry() {
        let expires_at = expiry_from_header("3d").unwrap();
        let hours = (expires_at - chrono::Utc::now()).num_hours();
        assert!((71..=72).contains(&hours));
    }

    #[test]
    fn parses_absolute_expiry() {
        assert_eq!(
            expiry_from_header("date=2030-01-02T03:04:05Z")
                .unwrap()
                .to_rfc3339(),
            "2030-01-02T03:04:05+00:00"
        );
    }
}
