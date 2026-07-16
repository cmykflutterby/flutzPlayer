use flutz_app::app::{
    decoded_project_title, merge_imported_track_metadata, merge_metadata_field_lists,
    track_metadata_from_imported_fields,
};
use flutz_fmid::{read_fmid, write_fmid, FmidFile, ProjectRecord};
use flutz_formats::{
    read_flutz_wrapper, write_flutz_wrapper, FlutzAudioWrapper, MetadataField, SourceAudioBlock,
    TrackMetadata,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let os_fields = vec![
        field("System.Title", "OS Title"),
        field("System.Music.Artist", "OS Artist"),
        field("System.Music.AlbumTitle", "OS Album"),
        field("System.Comment", "OS Notes"),
        field("System.CustomCue", "OS Custom"),
    ];
    let os_metadata = track_metadata_from_imported_fields(&os_fields);
    if os_metadata.project_name != "OS Title" {
        return Err("OS title did not map into project_name".into());
    }
    if os_metadata.artist != "OS Artist" {
        return Err("OS artist did not map into artist".into());
    }
    if os_metadata.album != "OS Album" {
        return Err("OS album title did not map into album".into());
    }
    if os_metadata.notes != "OS Notes" {
        return Err("OS comment did not map into notes".into());
    }
    if os_metadata.extra_fields != vec![field("System.CustomCue", "OS Custom")] {
        return Err("unmatched OS metadata was not retained as an extra field".into());
    }

    let embedded_metadata = TrackMetadata {
        project_name: "Embedded Title".to_owned(),
        artist: "Embedded Artist".to_owned(),
        source_filename: "source.mp3".to_owned(),
        extra_fields: vec![
            field("System.CustomCue", "Embedded Custom"),
            field("Encoder Flavor", "Symphonia Native"),
        ],
        ..TrackMetadata::default()
    };
    let merged = merge_imported_track_metadata(os_metadata, embedded_metadata);
    if merged.project_name != "Embedded Title" {
        return Err("embedded title did not override OS title".into());
    }
    if merged.artist != "Embedded Artist" {
        return Err("embedded artist did not override OS artist".into());
    }
    if merged.album != "OS Album" {
        return Err("OS album did not remain when embedded album was empty".into());
    }
    if merged.notes != "OS Notes" {
        return Err("OS notes did not remain when embedded notes were empty".into());
    }
    if merged
        .extra_fields
        .iter()
        .filter(|field| field.key == "System.CustomCue")
        .count()
        != 1
    {
        return Err("duplicate OS/native custom fields were not de-duplicated".into());
    }
    if merged.extra_fields[0].value != "Embedded Custom" {
        return Err("higher-priority custom field did not win de-duplication".into());
    }

    let title_from_os_when_embedded_title_missing = merge_imported_track_metadata(
        TrackMetadata {
            project_name: "OS Song Title".to_owned(),
            ..TrackMetadata::default()
        },
        TrackMetadata {
            source_filename: "filename-fallback.mp3".to_owned(),
            ..TrackMetadata::default()
        },
    );
    if title_from_os_when_embedded_title_missing.project_name != "OS Song Title" {
        return Err("filename fallback replaced OS title when embedded title was absent".into());
    }

    let title_from_native_when_flutz_title_missing = decoded_project_title(
        &TrackMetadata {
            source_filename: "filename-fallback.mp3".to_owned(),
            ..TrackMetadata::default()
        },
        &[field("title", "Native Song Title")],
        "filename-fallback",
    );
    if title_from_native_when_flutz_title_missing != "Native Song Title" {
        return Err("native title was not used before filename fallback".into());
    }

    let title_from_filename_when_metadata_missing = decoded_project_title(
        &TrackMetadata::default(),
        &[],
        "filename-fallback",
    );
    if title_from_filename_when_metadata_missing != "filename-fallback" {
        return Err("filename fallback was not used when no title metadata existed".into());
    }

    let native_fields = merge_metadata_field_lists(
        vec![field("Artist", "OS Artist"), field("Rating", "OS Rating")],
        vec![field("artist", "Embedded Artist"), field("Comment", "Embedded Comment")],
    );
    if native_fields != vec![field("artist", "Embedded Artist"), field("Comment", "Embedded Comment"), field("Rating", "OS Rating")] {
        return Err("native/imported field de-duplication did not preserve priority order".into());
    }

    let wrapper = FlutzAudioWrapper {
        source: SourceAudioBlock {
            format_id: "mp3".to_owned(),
            original_filename: "source.mp3".to_owned(),
            media_type: "audio/mpeg".to_owned(),
            bytes: b"encoded bytes".to_vec(),
        },
        metadata: TrackMetadata {
            project_name: "Wrapper Metadata".to_owned(),
            source_filename: "source.mp3".to_owned(),
            artist: "Wrapper Artist".to_owned(),
            album_artist: "Wrapper Album Artist".to_owned(),
            composer: "Wrapper Composer".to_owned(),
            conductor: "Wrapper Conductor".to_owned(),
            genre: "Wrapper Genre".to_owned(),
            date: "2026".to_owned(),
            track_number: "7".to_owned(),
            disc_total: "2".to_owned(),
            lyrics: "Wrapper lyrics".to_owned(),
            url: "https://example.invalid/wrapper".to_owned(),
            extra_fields: vec![field("Wrapper Native", "present")],
            ..TrackMetadata::default()
        },
        native_metadata: vec![field("id3:TBPM", "123")],
        ..FlutzAudioWrapper::default()
    };
    let decoded_wrapper = read_flutz_wrapper(&write_flutz_wrapper(&wrapper)?)?;
    if decoded_wrapper.metadata.artist != "Wrapper Artist"
        || decoded_wrapper.metadata.album_artist != "Wrapper Album Artist"
        || decoded_wrapper.metadata.lyrics != "Wrapper lyrics"
        || decoded_wrapper.metadata.extra_fields != vec![field("Wrapper Native", "present")]
        || decoded_wrapper.native_metadata != vec![field("id3:TBPM", "123")]
    {
        return Err("decoded wrapper metadata did not fully round trip".into());
    }

    let fmid = FmidFile {
        midi_bytes: b"MThd\0\0\0\x06\0\0\0\x01\0`MTrk\0\0\0\x04\0\xff/\0".to_vec(),
        project: ProjectRecord {
            project_name: "FMID Metadata".to_owned(),
            source_midi_filename: "source.mid".to_owned(),
            artist: "FMID Artist".to_owned(),
            album: "FMID Album".to_owned(),
            album_artist: "FMID Album Artist".to_owned(),
            composer: "FMID Composer".to_owned(),
            conductor: "FMID Conductor".to_owned(),
            genre: "FMID Genre".to_owned(),
            date: "2026".to_owned(),
            track_number: "3".to_owned(),
            track_total: "9".to_owned(),
            disc_number: "1".to_owned(),
            disc_total: "2".to_owned(),
            description: "FMID Description".to_owned(),
            copyright: "FMID Copyright".to_owned(),
            publisher: "FMID Publisher".to_owned(),
            encoded_by: "FMID Encoded By".to_owned(),
            encoder: "FMID Encoder".to_owned(),
            language: "en".to_owned(),
            lyrics: "FMID lyrics".to_owned(),
            url: "https://example.invalid/fmid".to_owned(),
            notes: "FMID Notes".to_owned(),
            extra_fields: vec![("FMID Extra".to_owned(), "present".to_owned())],
            ..ProjectRecord::default()
        },
        ..FmidFile::default()
    };
    let decoded_fmid = read_fmid(&write_fmid(&fmid))?;
    if decoded_fmid.project.artist != "FMID Artist"
        || decoded_fmid.project.album_artist != "FMID Album Artist"
        || decoded_fmid.project.lyrics != "FMID lyrics"
        || decoded_fmid.project.extra_fields != vec![("FMID Extra".to_owned(), "present".to_owned())]
    {
        return Err("FMID metadata did not fully round trip".into());
    }

    println!(
        "scenario=bug03-metadata-import merge=ok wrapper=ok fmid=ok extras={} status=ok",
        merged.extra_fields.len()
    );
    Ok(())
}

fn field(key: &str, value: &str) -> MetadataField {
    MetadataField {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}