use super::stream::stream_media_source;
use super::*;

pub fn read_track_metadata_with_symphonia(
    source: DecodedAudioStreamSource,
    format_id: &str,
    _project_name_fallback: &str,
    source_filename: &str,
) -> Result<(TrackMetadata, NativeMetadata)> {
    let (media_source, hint, _source_byte_len, _seekable) = stream_media_source(source)?;
    let media_source = MediaSourceStream::new(media_source, Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            media_source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| FlutzError::UnsupportedFormat(format!("probe failed: {error}")))?;
    let mut format = probed.format;
    let mut metadata_log = format.metadata();
    let tags = metadata_log
        .skip_to_latest()
        .map(|revision| revision.tags())
        .unwrap_or(&[]);
    Ok(extract_track_and_native_metadata(
        tags,
        format_id,
        source_filename,
    ))
}

fn extract_track_and_native_metadata(
    tags: &[Tag],
    format_id: &str,
    source_filename: &str,
) -> (TrackMetadata, NativeMetadata) {
    let mut metadata = TrackMetadata {
        source_filename: source_filename.to_owned(),
        ..TrackMetadata::default()
    };
    let mut native_metadata = Vec::new();

    for tag in tags {
        let value = tag.value.to_string();
        if value.trim().is_empty() {
            continue;
        }
        if apply_standard_tag(&mut metadata, tag.std_key, &value)
            || apply_tag_key_hint(&mut metadata, &tag.key, &value)
        {
            continue;
        }
        native_metadata.push(MetadataField {
            key: if tag.key.trim().is_empty() {
                tag.std_key
                    .map(standard_tag_key_name)
                    .unwrap_or("tag")
                    .to_owned()
            } else {
                tag.key.clone()
            },
            value,
        });
    }

    let _ = format_id;
    if metadata.source_filename.trim().is_empty() {
        metadata.source_filename = source_filename.to_owned();
    }

    (metadata, native_metadata)
}

fn apply_standard_tag(
    metadata: &mut TrackMetadata,
    standard_key: Option<StandardTagKey>,
    value: &str,
) -> bool {
    let Some(standard_key) = standard_key else {
        return false;
    };

    match standard_key {
        StandardTagKey::TrackTitle => set_if_empty(&mut metadata.project_name, value),
        StandardTagKey::Artist => set_if_empty(&mut metadata.artist, value),
        StandardTagKey::Album => set_if_empty(&mut metadata.album, value),
        StandardTagKey::AlbumArtist => set_if_empty(&mut metadata.album_artist, value),
        StandardTagKey::Composer => set_if_empty(&mut metadata.composer, value),
        StandardTagKey::Conductor => set_if_empty(&mut metadata.conductor, value),
        StandardTagKey::Genre => set_if_empty(&mut metadata.genre, value),
        StandardTagKey::Date
        | StandardTagKey::ReleaseDate
        | StandardTagKey::OriginalDate
        | StandardTagKey::EncodingDate => set_if_empty(&mut metadata.date, value),
        StandardTagKey::TrackNumber => set_if_empty(&mut metadata.track_number, value),
        StandardTagKey::TrackTotal => set_if_empty(&mut metadata.track_total, value),
        StandardTagKey::DiscNumber => set_if_empty(&mut metadata.disc_number, value),
        StandardTagKey::DiscTotal => set_if_empty(&mut metadata.disc_total, value),
        StandardTagKey::Description => set_if_empty(&mut metadata.description, value),
        StandardTagKey::Comment => set_if_empty(&mut metadata.notes, value),
        StandardTagKey::Copyright => set_if_empty(&mut metadata.copyright, value),
        StandardTagKey::Label => set_if_empty(&mut metadata.publisher, value),
        StandardTagKey::EncodedBy => set_if_empty(&mut metadata.encoded_by, value),
        StandardTagKey::Encoder => set_if_empty(&mut metadata.encoder, value),
        StandardTagKey::Language => set_if_empty(&mut metadata.language, value),
        StandardTagKey::Lyrics => set_if_empty(&mut metadata.lyrics, value),
        StandardTagKey::Url | StandardTagKey::UrlOfficial | StandardTagKey::UrlSource => {
            set_if_empty(&mut metadata.url, value)
        }
        _ => false,
    }
}

fn apply_tag_key_hint(metadata: &mut TrackMetadata, key: &str, value: &str) -> bool {
    let normalized = key
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '/'], "_");

    match normalized.as_str() {
        "title" | "tracktitle" | "track_title" => set_if_empty(&mut metadata.project_name, value),
        "artist" => set_if_empty(&mut metadata.artist, value),
        "album" => set_if_empty(&mut metadata.album, value),
        "albumartist" | "album_artist" => set_if_empty(&mut metadata.album_artist, value),
        "composer" => set_if_empty(&mut metadata.composer, value),
        "conductor" => set_if_empty(&mut metadata.conductor, value),
        "genre" => set_if_empty(&mut metadata.genre, value),
        "date" | "year" | "releasedate" | "release_date" => set_if_empty(&mut metadata.date, value),
        "tracknumber" | "track_number" => set_if_empty(&mut metadata.track_number, value),
        "tracktotal" | "track_total" | "totaltracks" => {
            set_if_empty(&mut metadata.track_total, value)
        }
        "discnumber" | "disc_number" => set_if_empty(&mut metadata.disc_number, value),
        "disctotal" | "disc_total" | "totaldiscs" => set_if_empty(&mut metadata.disc_total, value),
        "description" => set_if_empty(&mut metadata.description, value),
        "comment" | "comments" => set_if_empty(&mut metadata.notes, value),
        "copyright" => set_if_empty(&mut metadata.copyright, value),
        "publisher" | "label" => set_if_empty(&mut metadata.publisher, value),
        "encodedby" | "encoded_by" => set_if_empty(&mut metadata.encoded_by, value),
        "encoder" => set_if_empty(&mut metadata.encoder, value),
        "language" => set_if_empty(&mut metadata.language, value),
        "lyrics" => set_if_empty(&mut metadata.lyrics, value),
        "url" | "website" => set_if_empty(&mut metadata.url, value),
        _ => false,
    }
}

fn set_if_empty(slot: &mut String, value: &str) -> bool {
    if slot.trim().is_empty() {
        *slot = value.to_owned();
        true
    } else {
        false
    }
}

fn standard_tag_key_name(standard_key: StandardTagKey) -> &'static str {
    match standard_key {
        StandardTagKey::TrackTitle => "TrackTitle",
        StandardTagKey::Artist => "Artist",
        StandardTagKey::Album => "Album",
        StandardTagKey::AlbumArtist => "AlbumArtist",
        StandardTagKey::Composer => "Composer",
        StandardTagKey::Conductor => "Conductor",
        StandardTagKey::Genre => "Genre",
        StandardTagKey::Date => "Date",
        StandardTagKey::ReleaseDate => "ReleaseDate",
        StandardTagKey::OriginalDate => "OriginalDate",
        StandardTagKey::EncodingDate => "EncodingDate",
        StandardTagKey::TrackNumber => "TrackNumber",
        StandardTagKey::TrackTotal => "TrackTotal",
        StandardTagKey::DiscNumber => "DiscNumber",
        StandardTagKey::DiscTotal => "DiscTotal",
        StandardTagKey::Description => "Description",
        StandardTagKey::Comment => "Comment",
        StandardTagKey::Copyright => "Copyright",
        StandardTagKey::Label => "Label",
        StandardTagKey::EncodedBy => "EncodedBy",
        StandardTagKey::Encoder => "Encoder",
        StandardTagKey::Language => "Language",
        StandardTagKey::Lyrics => "Lyrics",
        StandardTagKey::Url => "Url",
        StandardTagKey::UrlOfficial => "UrlOfficial",
        StandardTagKey::UrlSource => "UrlSource",
        _ => "tag",
    }
}
