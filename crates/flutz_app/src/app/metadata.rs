use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataEditState {
    pub project_name: String,
    pub source_filename: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub composer: String,
    pub conductor: String,
    pub genre: String,
    pub date: String,
    pub track_number: String,
    pub track_total: String,
    pub disc_number: String,
    pub disc_total: String,
    pub description: String,
    pub copyright: String,
    pub publisher: String,
    pub encoded_by: String,
    pub encoder: String,
    pub language: String,
    pub lyrics: String,
    pub url: String,
    pub notes: String,
    pub extra_fields: Vec<MetadataField>,
    pub native_metadata: NativeMetadata,
    pub supports_native_metadata: bool,
}

impl MetadataEditState {
    pub(super) fn from_track_metadata(
        metadata: &TrackMetadata,
        fallback_project_name: String,
        fallback_source_filename: String,
    ) -> Self {
        Self {
            project_name: if metadata.project_name.trim().is_empty() {
                fallback_project_name
            } else {
                metadata.project_name.clone()
            },
            source_filename: if metadata.source_filename.trim().is_empty() {
                fallback_source_filename
            } else {
                metadata.source_filename.clone()
            },
            artist: metadata.artist.clone(),
            album: metadata.album.clone(),
            album_artist: metadata.album_artist.clone(),
            composer: metadata.composer.clone(),
            conductor: metadata.conductor.clone(),
            genre: metadata.genre.clone(),
            date: metadata.date.clone(),
            track_number: metadata.track_number.clone(),
            track_total: metadata.track_total.clone(),
            disc_number: metadata.disc_number.clone(),
            disc_total: metadata.disc_total.clone(),
            description: metadata.description.clone(),
            copyright: metadata.copyright.clone(),
            publisher: metadata.publisher.clone(),
            encoded_by: metadata.encoded_by.clone(),
            encoder: metadata.encoder.clone(),
            language: metadata.language.clone(),
            lyrics: metadata.lyrics.clone(),
            url: metadata.url.clone(),
            notes: metadata.notes.clone(),
            extra_fields: metadata.extra_fields.clone(),
            native_metadata: Vec::new(),
            supports_native_metadata: false,
        }
    }

    pub(super) fn from_fmid_project(
        project: &FmidProjectRecord,
        fallback_project_name: String,
        fallback_source_filename: String,
    ) -> Self {
        Self {
            project_name: if project.project_name.trim().is_empty() {
                fallback_project_name
            } else {
                project.project_name.clone()
            },
            source_filename: if project.source_midi_filename.trim().is_empty() {
                fallback_source_filename
            } else {
                project.source_midi_filename.clone()
            },
            artist: project.artist.clone(),
            album: project.album.clone(),
            album_artist: project.album_artist.clone(),
            composer: project.composer.clone(),
            conductor: project.conductor.clone(),
            genre: project.genre.clone(),
            date: project.date.clone(),
            track_number: project.track_number.clone(),
            track_total: project.track_total.clone(),
            disc_number: project.disc_number.clone(),
            disc_total: project.disc_total.clone(),
            description: project.description.clone(),
            copyright: project.copyright.clone(),
            publisher: project.publisher.clone(),
            encoded_by: project.encoded_by.clone(),
            encoder: project.encoder.clone(),
            language: project.language.clone(),
            lyrics: project.lyrics.clone(),
            url: project.url.clone(),
            notes: project.notes.clone(),
            extra_fields: project
                .extra_fields
                .iter()
                .map(|(key, value)| MetadataField {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            native_metadata: Vec::new(),
            supports_native_metadata: false,
        }
    }

    pub(super) fn with_native_metadata(mut self, native_metadata: NativeMetadata) -> Self {
        self.native_metadata = native_metadata;
        self.supports_native_metadata = true;
        self
    }

    pub(super) fn to_track_metadata(
        &self,
        fallback_project_name: &str,
        fallback_source_filename: &str,
    ) -> TrackMetadata {
        TrackMetadata {
            project_name: first_non_empty(&self.project_name, fallback_project_name),
            source_filename: first_non_empty(&self.source_filename, fallback_source_filename),
            artist: self.artist.clone(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            composer: self.composer.clone(),
            conductor: self.conductor.clone(),
            genre: self.genre.clone(),
            date: self.date.clone(),
            track_number: self.track_number.clone(),
            track_total: self.track_total.clone(),
            disc_number: self.disc_number.clone(),
            disc_total: self.disc_total.clone(),
            description: self.description.clone(),
            copyright: self.copyright.clone(),
            publisher: self.publisher.clone(),
            encoded_by: self.encoded_by.clone(),
            encoder: self.encoder.clone(),
            language: self.language.clone(),
            lyrics: self.lyrics.clone(),
            url: self.url.clone(),
            notes: self.notes.clone(),
            extra_fields: normalized_metadata_fields(&self.extra_fields),
        }
    }

    pub(super) fn to_fmid_project(
        &self,
        fallback_project_name: &str,
        fallback_source_filename: &str,
    ) -> FmidProjectRecord {
        FmidProjectRecord {
            project_name: first_non_empty(&self.project_name, fallback_project_name),
            source_midi_filename: first_non_empty(&self.source_filename, fallback_source_filename),
            artist: self.artist.clone(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            composer: self.composer.clone(),
            conductor: self.conductor.clone(),
            genre: self.genre.clone(),
            date: self.date.clone(),
            track_number: self.track_number.clone(),
            track_total: self.track_total.clone(),
            disc_number: self.disc_number.clone(),
            disc_total: self.disc_total.clone(),
            description: self.description.clone(),
            copyright: self.copyright.clone(),
            publisher: self.publisher.clone(),
            encoded_by: self.encoded_by.clone(),
            encoder: self.encoder.clone(),
            language: self.language.clone(),
            lyrics: self.lyrics.clone(),
            url: self.url.clone(),
            project_flags: 0,
            notes: self.notes.clone(),
            extra_fields: normalized_metadata_fields(&self.extra_fields)
                .into_iter()
                .map(|field| (field.key, field.value))
                .collect(),
        }
    }
}

fn first_non_empty(primary: &str, fallback: &str) -> String {
    if primary.trim().is_empty() {
        fallback.to_owned()
    } else {
        primary.to_owned()
    }
}

pub(super) fn normalized_metadata_fields(fields: &[MetadataField]) -> Vec<MetadataField> {
    fields
        .iter()
        .filter_map(|field| {
            let key = field.key.trim();
            let value = field.value.trim();
            if key.is_empty() && value.is_empty() {
                None
            } else {
                Some(MetadataField {
                    key: key.to_owned(),
                    value: value.to_owned(),
                })
            }
        })
        .collect()
}

pub fn merge_imported_track_metadata(
    mut lower_priority: TrackMetadata,
    higher_priority: TrackMetadata,
) -> TrackMetadata {
    overlay_non_empty(
        &mut lower_priority.project_name,
        higher_priority.project_name,
    );
    overlay_non_empty(
        &mut lower_priority.source_filename,
        higher_priority.source_filename,
    );
    overlay_non_empty(&mut lower_priority.artist, higher_priority.artist);
    overlay_non_empty(&mut lower_priority.album, higher_priority.album);
    overlay_non_empty(
        &mut lower_priority.album_artist,
        higher_priority.album_artist,
    );
    overlay_non_empty(&mut lower_priority.composer, higher_priority.composer);
    overlay_non_empty(&mut lower_priority.conductor, higher_priority.conductor);
    overlay_non_empty(&mut lower_priority.genre, higher_priority.genre);
    overlay_non_empty(&mut lower_priority.date, higher_priority.date);
    overlay_non_empty(
        &mut lower_priority.track_number,
        higher_priority.track_number,
    );
    overlay_non_empty(&mut lower_priority.track_total, higher_priority.track_total);
    overlay_non_empty(&mut lower_priority.disc_number, higher_priority.disc_number);
    overlay_non_empty(&mut lower_priority.disc_total, higher_priority.disc_total);
    overlay_non_empty(&mut lower_priority.description, higher_priority.description);
    overlay_non_empty(&mut lower_priority.copyright, higher_priority.copyright);
    overlay_non_empty(&mut lower_priority.publisher, higher_priority.publisher);
    overlay_non_empty(&mut lower_priority.encoded_by, higher_priority.encoded_by);
    overlay_non_empty(&mut lower_priority.encoder, higher_priority.encoder);
    overlay_non_empty(&mut lower_priority.language, higher_priority.language);
    overlay_non_empty(&mut lower_priority.lyrics, higher_priority.lyrics);
    overlay_non_empty(&mut lower_priority.url, higher_priority.url);
    overlay_non_empty(&mut lower_priority.notes, higher_priority.notes);
    lower_priority.extra_fields =
        merge_metadata_field_lists(lower_priority.extra_fields, higher_priority.extra_fields);
    lower_priority
}

fn overlay_non_empty(slot: &mut String, value: String) {
    if !value.trim().is_empty() {
        *slot = value;
    }
}

pub fn merge_metadata_field_lists(
    lower_priority: Vec<MetadataField>,
    higher_priority: Vec<MetadataField>,
) -> Vec<MetadataField> {
    let mut merged = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    for field in higher_priority.into_iter().chain(lower_priority) {
        let key = field.key.trim();
        let value = field.value.trim();
        if key.is_empty() && value.is_empty() {
            continue;
        }
        let normalized_key = metadata_field_key(key);
        if seen.insert(normalized_key) {
            merged.push(MetadataField {
                key: key.to_owned(),
                value: value.to_owned(),
            });
        }
    }
    merged
}

fn metadata_field_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '/', '.'], "_")
}

fn apply_imported_metadata_field(
    metadata: &mut TrackMetadata,
    extras: &mut Vec<MetadataField>,
    key: &str,
    value: String,
) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    match metadata_field_key(key).as_str() {
        "system_title" | "title" | "tracktitle" | "track_title" => {
            metadata.project_name = value.to_owned()
        }
        "system_music_artist" | "artist" => metadata.artist = value.to_owned(),
        "system_music_albumtitle" | "system_music_album" | "album" => {
            metadata.album = value.to_owned()
        }
        "system_music_albumartist" | "albumartist" | "album_artist" => {
            metadata.album_artist = value.to_owned()
        }
        "system_music_composer" | "composer" => metadata.composer = value.to_owned(),
        "system_music_conductor" | "conductor" => metadata.conductor = value.to_owned(),
        "system_music_genre" | "genre" => metadata.genre = value.to_owned(),
        "system_media_datereleased" | "system_itemdate" | "date" | "year" => {
            metadata.date = value.to_owned()
        }
        "system_music_tracknumber" | "tracknumber" | "track_number" => {
            metadata.track_number = value.to_owned()
        }
        "system_music_trackcount" | "tracktotal" | "track_total" | "totaltracks" => {
            metadata.track_total = value.to_owned()
        }
        "system_music_partofset" | "discnumber" | "disc_number" => {
            metadata.disc_number = value.to_owned()
        }
        "system_comment" | "comment" | "comments" => metadata.notes = value.to_owned(),
        "system_copyright" | "copyright" => metadata.copyright = value.to_owned(),
        "system_company" | "system_music_publisher" | "publisher" | "label" => {
            metadata.publisher = value.to_owned()
        }
        "system_encodedby" | "encodedby" | "encoded_by" => metadata.encoded_by = value.to_owned(),
        "system_media_encodingtool" | "encoder" => metadata.encoder = value.to_owned(),
        "system_language" | "language" => metadata.language = value.to_owned(),
        "system_music_lyrics" | "lyrics" => metadata.lyrics = value.to_owned(),
        "system_link_targeturl" | "url" | "website" => metadata.url = value.to_owned(),
        _ => extras.push(MetadataField {
            key: key.to_owned(),
            value: value.to_owned(),
        }),
    }
}

pub fn track_metadata_from_imported_fields(fields: &[MetadataField]) -> TrackMetadata {
    let mut metadata = TrackMetadata::default();
    let mut extras = Vec::new();
    for field in fields {
        apply_imported_metadata_field(
            &mut metadata,
            &mut extras,
            field.key.as_str(),
            field.value.clone(),
        );
    }
    metadata.extra_fields = merge_metadata_field_lists(Vec::new(), extras);
    metadata
}

pub fn decoded_project_title(
    metadata: &TrackMetadata,
    native_metadata: &[MetadataField],
    fallback_project_name: &str,
) -> String {
    if !metadata.project_name.trim().is_empty() {
        return metadata.project_name.clone();
    }
    let imported = track_metadata_from_imported_fields(native_metadata);
    if !imported.project_name.trim().is_empty() {
        return imported.project_name;
    }
    fallback_project_name.to_owned()
}

pub(super) fn read_os_track_metadata(path: &Path) -> (TrackMetadata, Vec<MetadataField>) {
    read_platform_os_track_metadata(path).unwrap_or_default()
}

#[cfg(not(windows))]
fn read_platform_os_track_metadata(_path: &Path) -> Option<(TrackMetadata, Vec<MetadataField>)> {
    None
}

#[cfg(windows)]
fn read_platform_os_track_metadata(path: &Path) -> Option<(TrackMetadata, Vec<MetadataField>)> {
    use windows::{
        core::HSTRING,
        Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            System::{
                Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
                Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
            },
            UI::Shell::PropertiesSystem::{
                IPropertyStore, PSGetPropertyKeyFromName, SHGetPropertyStoreFromParsingName,
                GPS_BESTEFFORT,
            },
        },
    };

    struct ComApartment {
        uninitialize: bool,
    }
    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.uninitialize {
                unsafe { CoUninitialize() };
            }
        }
    }

    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let apartment = if init.is_ok() {
        ComApartment { uninitialize: true }
    } else if init == RPC_E_CHANGED_MODE {
        ComApartment {
            uninitialize: false,
        }
    } else {
        return None;
    };

    let path_string = path.as_os_str().to_string_lossy();
    let path_hstring = HSTRING::from(path_string.as_ref());
    let store = unsafe {
        SHGetPropertyStoreFromParsingName::<
            _,
            Option<&windows::Win32::System::Com::IBindCtx>,
            IPropertyStore,
        >(&path_hstring, None, GPS_BESTEFFORT)
    }
    .ok()?;

    let mut metadata = TrackMetadata::default();
    let mut extras = Vec::new();
    for property_name in OS_METADATA_PROPERTY_NAMES {
        let name = HSTRING::from(*property_name);
        let mut key = Default::default();
        if unsafe { PSGetPropertyKeyFromName(&name, &mut key) }.is_err() {
            continue;
        }
        let mut value = match unsafe { store.GetValue(&key) } {
            Ok(value) => value,
            Err(_) => continue,
        };
        let text = unsafe { PropVariantToStringAlloc(&value) }
            .ok()
            .and_then(|wide| {
                let output = pwstr_to_string(wide);
                unsafe { CoTaskMemFree(Some(wide.0.cast())) };
                output
            });
        let _ = unsafe { PropVariantClear(&mut value) };
        if let Some(text) = text {
            apply_imported_metadata_field(&mut metadata, &mut extras, property_name, text);
        }
    }
    drop(apartment);
    Some((metadata, extras))
}

#[cfg(windows)]
fn pwstr_to_string(value: windows::core::PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut len = 0usize;
    unsafe {
        while *value.0.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(value.0, len));
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

const OS_METADATA_PROPERTY_NAMES: &[&str] = &[
    "System.Title",
    "System.Music.Artist",
    "System.Music.AlbumTitle",
    "System.Music.AlbumArtist",
    "System.Music.Composer",
    "System.Music.Conductor",
    "System.Music.Genre",
    "System.Media.DateReleased",
    "System.ItemDate",
    "System.Music.TrackNumber",
    "System.Music.TrackCount",
    "System.Music.PartOfSet",
    "System.Comment",
    "System.Copyright",
    "System.Company",
    "System.Music.Publisher",
    "System.EncodedBy",
    "System.Media.EncodingTool",
    "System.Language",
    "System.Music.Lyrics",
    "System.Link.TargetUrl",
];

pub(super) fn draw_metadata_text_field(ui: &mut egui::Ui, label: &str, value: &mut String) -> bool {
    ui.label(label);
    ui.text_edit_singleline(value).changed()
}

pub(super) fn draw_metadata_multiline_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    rows: usize,
) -> bool {
    ui.label(label);
    ui.add(egui::TextEdit::multiline(value).desired_rows(rows))
        .changed()
}

pub(super) fn draw_metadata_field_list(
    ui: &mut egui::Ui,
    heading: &str,
    add_label: &str,
    fields: &mut Vec<MetadataField>,
) -> bool {
    let mut changed = false;
    ui.label(heading);
    let mut remove_index = None;
    for (index, field) in fields.iter_mut().enumerate() {
        ui.push_id((heading, index), |ui| {
            ui.horizontal(|ui| {
                changed |= ui.text_edit_singleline(&mut field.key).changed();
                changed |= ui.text_edit_singleline(&mut field.value).changed();
                if ui.button("x").clicked() {
                    remove_index = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove_index {
        fields.remove(index);
        changed = true;
    }
    if ui.button(add_label).clicked() {
        fields.push(MetadataField::default());
        changed = true;
    }
    changed
}
