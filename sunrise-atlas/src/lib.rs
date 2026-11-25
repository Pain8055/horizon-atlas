//! Atlas World Map, Minimap & Waypoints UI (v3) for Sunrise / Starling Preview 4.
//! Pure `sunrise-mod-api` native mod (Compile v2 compatible: no mixins, no extra crates).

use mod_api::client::widgets::{Button, Outcome, TextField, WidgetId, Widgets};
use mod_api::{
    ClientHost, ClientTickPostEvent, CommandEvent, Flow, GuiRenderPostEvent, Hooks,
    HostBlockStates, Identifier, InputKeyEvent, KeyAction, ScreenCharTypedPreEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseButtonReleasedPreEvent, ScreenMouseDraggedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, Text,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const ID: &str = "atlas";
const MINIMAP_TEX: &str = "atlas:dynamic/minimap";
const WORLDMAP_TEX: &str = "atlas:dynamic/worldmap";

const MINI_SIZE: u32 = 128;
const WORLD_SIZE: u32 = 512;
const CHUNK_SCAN_BUDGET: usize = 2;
const WPS_PER_PAGE: usize = 5;

const WP_COLORS: [(&str, [u8; 4], u32); 6] = [
    ("Red", [255, 85, 85, 255], 0xFFFF_5555),
    ("Cyan", [85, 230, 255, 255], 0xFF55_E6FF),
    ("Green", [85, 235, 110, 255], 0xFF55_EB6E),
    ("Yellow", [255, 225, 85, 255], 0xFFFF_E155),
    ("Purple", [205, 120, 255, 255], 0xFFCD_78FF),
    ("White", [255, 255, 255, 255], 0xFFFF_FFFF),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveView {
    WorldMap,
    Waypoints,
}

#[derive(Clone)]
struct Waypoint {
    name: String,
    x: i32,
    y: i32,
    z: i32,
    color_idx: usize,
    visible: bool,
}

#[derive(Clone)]
struct ChunkTile {
    rgba: [u8; 16 * 16 * 4],
}

struct MapState {
    tiles: HashMap<(i32, i32), ChunkTile>,
    waypoints: Vec<Waypoint>,
    player_x: f64,
    player_y: f64,
    player_z: f64,
    view_x: f64,
    view_z: f64,
    zoom: f64,
    minimap_enabled: bool,
    mini_created: bool,
    world_created: bool,
    world_dirty: bool,
    mini_rgba: Vec<u8>,
    world_rgba: Vec<u8>,
    dragging: bool,
    last_mouse: [f32; 2],
    tick_counter: u64,
    scan_ring_idx: usize,
    active_view: ActiveView,
    wp_page: usize,
    wp_color_idx: usize,
    wp_draft_name: String,
    ui_rev: u64,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            tiles: HashMap::new(),
            waypoints: Vec::new(),
            player_x: 0.0,
            player_y: 64.0,
            player_z: 0.0,
            view_x: 0.0,
            view_z: 0.0,
            zoom: 1.0,
            minimap_enabled: true,
            mini_created: false,
            world_created: false,
            world_dirty: true,
            mini_rgba: vec![20u8; (MINI_SIZE * MINI_SIZE * 4) as usize],
            world_rgba: vec![16u8; (WORLD_SIZE * WORLD_SIZE * 4) as usize],
            dragging: false,
            last_mouse: [0.0, 0.0],
            tick_counter: 0,
            scan_ring_idx: 0,
            active_view: ActiveView::WorldMap,
            wp_page: 0,
            wp_color_idx: 0,
            wp_draft_name: String::new(),
            ui_rev: 0,
        }
    }
}

struct WorldMapUi {
    widgets: Widgets,
    zoom_in: WidgetId,
    zoom_out: WidgetId,
    center: WidgetId,
    add_wp: WidgetId,
    open_wps: WidgetId,
    toggle_mini: WidgetId,
    close: WidgetId,
}

struct WpRowButtons {
    wp_index: usize,
    goto_btn: WidgetId,
    vis_btn: WidgetId,
    del_btn: WidgetId,
    y: f32,
}

struct WaypointsUi {
    widgets: Widgets,
    name_field: WidgetId,
    color_btn: WidgetId,
    add_btn: WidgetId,
    rows: Vec<WpRowButtons>,
    prev_btn: WidgetId,
    next_btn: WidgetId,
    map_btn: WidgetId,
    close_btn: WidgetId,
}

enum ScreenUiKind {
    WorldMap(WorldMapUi),
    Waypoints(WaypointsUi),
}

struct ScreenUi {
    width: f32,
    height: f32,
    view: ActiveView,
    ui_rev: u64,
    kind: ScreenUiKind,
}

impl ScreenUi {
    fn build(width: f32, height: f32, st: &MapState) -> Self {
        let kind = match st.active_view {
            ActiveView::WorldMap => {
                let mut widgets = Widgets::new();
                let y = (height - 28.0).max(8.0);
                let zoom_in =
                    widgets.add_button(Button::new([6.0, y, 26.0, 22.0], Text::literal("+")));
                let zoom_out =
                    widgets.add_button(Button::new([35.0, y, 26.0, 22.0], Text::literal("-")));
                let center =
                    widgets.add_button(Button::new([64.0, y, 50.0, 22.0], Text::literal("Center")));
                let add_wp =
                    widgets.add_button(Button::new([117.0, y, 38.0, 22.0], Text::literal("+WP")));
                let open_wps =
                    widgets.add_button(Button::new([158.0, y, 42.0, 22.0], Text::literal("WPs")));
                let mini_label = if st.minimap_enabled {
                    "Mini:ON"
                } else {
                    "Mini:OFF"
                };
                let toggle_mini = widgets.add_button(Button::new(
                    [203.0, y, 64.0, 22.0],
                    Text::literal(mini_label),
                ));
                let close = widgets.add_button(Button::new(
                    [(width - 54.0).max(270.0), y, 48.0, 22.0],
                    Text::literal("Close"),
                ));
                ScreenUiKind::WorldMap(WorldMapUi {
                    widgets,
                    zoom_in,
                    zoom_out,
                    center,
                    add_wp,
                    open_wps,
                    toggle_mini,
                    close,
                })
            }
            ActiveView::Waypoints => {
                let mut widgets = Widgets::new();
                let pad = 12.0f32;
                let top_y = 26.0f32;
                let field_w = (width - 178.0).clamp(90.0, 260.0);
                let mut tf = TextField::new([pad, top_y, field_w, 20.0], 24);
                tf.set_value(&st.wp_draft_name);
                let name_field = widgets.add_text_field(tf);

                let (color_name, _, _) = WP_COLORS[st.wp_color_idx % WP_COLORS.len()];
                let color_btn = widgets.add_button(Button::new(
                    [pad + field_w + 6.0, top_y, 64.0, 20.0],
                    Text::literal(color_name),
                ));
                let add_btn = widgets.add_button(Button::new(
                    [pad + field_w + 74.0, top_y, 76.0, 20.0],
                    Text::literal("+ Add Here"),
                ));

                let mut rows = Vec::new();
                let start = st.wp_page * WPS_PER_PAGE;
                let end = (start + WPS_PER_PAGE).min(st.waypoints.len());
                let btn_right = (width - pad).max(240.0);
                for (slot, idx) in (start..end).enumerate() {
                    let ry = 56.0 + (slot as f32) * 26.0;
                    let vis_label = if st.waypoints[idx].visible { "ON" } else { "OFF" };
                    let goto_btn = widgets.add_button(Button::new(
                        [btn_right - 122.0, ry, 40.0, 20.0],
                        Text::literal("Map"),
                    ));
                    let vis_btn = widgets.add_button(Button::new(
                        [btn_right - 78.0, ry, 36.0, 20.0],
                        Text::literal(vis_label),
                    ));
                    let del_btn = widgets.add_button(Button::new(
                        [btn_right - 38.0, ry, 36.0, 20.0],
                        Text::literal("Del"),
                    ));
                    rows.push(WpRowButtons {
                        wp_index: idx,
                        goto_btn,
                        vis_btn,
                        del_btn,
                        y: ry,
                    });
                }

                let bot_y = (height - 28.0).max(190.0);
                let prev_btn =
                    widgets.add_button(Button::new([pad, bot_y, 50.0, 22.0], Text::literal("Prev")));
                let next_btn = widgets.add_button(Button::new(
                    [pad + 54.0, bot_y, 50.0, 22.0],
                    Text::literal("Next"),
                ));
                let map_btn = widgets.add_button(Button::new(
                    [pad + 112.0, bot_y, 74.0, 22.0],
                    Text::literal("World Map"),
                ));
                let close_btn = widgets.add_button(Button::new(
                    [(width - 62.0).max(200.0), bot_y, 50.0, 22.0],
                    Text::literal("Close"),
                ));

                ScreenUiKind::Waypoints(WaypointsUi {
                    widgets,
                    name_field,
                    color_btn,
                    add_btn,
                    rows,
                    prev_btn,
                    next_btn,
                    map_btn,
                    close_btn,
                })
            }
        };

        Self {
            width,
            height,
            view: st.active_view,
            ui_rev: st.ui_rev,
            kind,
        }
    }

    fn widgets_mut(&mut self) -> &mut Widgets {
        match &mut self.kind {
            ScreenUiKind::WorldMap(u) => &mut u.widgets,
            ScreenUiKind::Waypoints(u) => &mut u.widgets,
        }
    }
}

#[derive(Default)]
struct AtlasMod {
    state: Mutex<MapState>,
    ui: Mutex<Option<ScreenUi>>,
    open_req: AtomicBool,
    close_req: AtomicBool,
}

impl AtlasMod {
    fn open_view(&self, view: ActiveView) {
        {
            let mut st = self.state.lock().unwrap();
            st.active_view = view;
            if view == ActiveView::WorldMap {
                st.view_x = st.player_x;
                st.view_z = st.player_z;
                st.world_dirty = true;
            }
            st.ui_rev = st.ui_rev.wrapping_add(1);
        }
        self.open_req.store(true, Ordering::Relaxed);
    }

    fn scan_chunk(
        host: &dyn ClientHost,
        level: mod_api::LevelRef,
        cx: i32,
        cz: i32,
    ) -> Option<ChunkTile> {
        let states = host.block_states();
        let mut rgba = [0u8; 16 * 16 * 4];
        let mut found_any = false;
        let base_x = cx * 16;
        let base_z = cz * 16;

        for dz in 0..16i32 {
            for dx in 0..16i32 {
                let wx = base_x + dx;
                let wz = base_z + dz;
                let mut col = [24u8, 24, 32, 255];
                let mut top_y = 64i32;

                for y in (-64..=256i32).rev() {
                    if let Some(bs) = level.block_state(host, [wx, y, wz]) {
                        if let Some(packed) = states.map_color(bs) {
                            if packed != 0 {
                                if let Some(c) = states.map_color_rgba(packed) {
                                    col = c;
                                    top_y = y;
                                    found_any = true;
                                    break;
                                }
                            }
                        }
                    }
                }

                let shade = ((top_y as f32 - 64.0) / 140.0).clamp(-0.22, 0.22);
                let mul = 1.0 + shade;
                let idx = ((dz as usize) * 16 + (dx as usize)) * 4;
                rgba[idx] = ((col[0] as f32) * mul).clamp(0.0, 255.0) as u8;
                rgba[idx + 1] = ((col[1] as f32) * mul).clamp(0.0, 255.0) as u8;
                rgba[idx + 2] = ((col[2] as f32) * mul).clamp(0.0, 255.0) as u8;
                rgba[idx + 3] = 255;
            }
        }

        if found_any {
            Some(ChunkTile { rgba })
        } else {
            None
        }
    }

    fn render_buffer(
        tiles: &HashMap<(i32, i32), ChunkTile>,
        waypoints: &[Waypoint],
        center_x: f64,
        center_z: f64,
        player_x: f64,
        player_z: f64,
        blocks_per_px: f64,
        size: u32,
        out: &mut [u8],
    ) {
        let half = (size as f64) * 0.5;
        for py in 0..size {
            let wz = (center_z + (py as f64 - half) * blocks_per_px).floor() as i32;
            let cz = wz.div_euclid(16);
            let lz = wz.rem_euclid(16) as usize;
            for px in 0..size {
                let wx = (center_x + (px as f64 - half) * blocks_per_px).floor() as i32;
                let cx = wx.div_euclid(16);
                let lx = wx.rem_euclid(16) as usize;
                let o = ((py * size + px) * 4) as usize;

                if let Some(tile) = tiles.get(&(cx, cz)) {
                    let t = (lz * 16 + lx) * 4;
                    out[o..o + 4].copy_from_slice(&tile.rgba[t..t + 4]);
                } else {
                    let grid = (((wx >> 4) + (wz >> 4)) & 1) == 0;
                    let v = if grid { 22 } else { 18 };
                    out[o] = v;
                    out[o + 1] = v;
                    out[o + 2] = v + 6;
                    out[o + 3] = 235;
                }
            }
        }

        for wp in waypoints {
            if !wp.visible {
                continue;
            }
            let sx = ((wp.x as f64 - center_x) / blocks_per_px + half).round() as i32;
            let sy = ((wp.z as f64 - center_z) / blocks_per_px + half).round() as i32;
            let (_, rgba, _) = WP_COLORS[wp.color_idx % WP_COLORS.len()];
            Self::draw_dot(out, size, sx, sy, 3, rgba);
        }

        let psx = ((player_x - center_x) / blocks_per_px + half).round() as i32;
        let psy = ((player_z - center_z) / blocks_per_px + half).round() as i32;
        Self::draw_dot(out, size, psx, psy, 3, [255, 255, 255, 255]);
        Self::draw_dot(out, size, psx, psy, 1, [60, 210, 255, 255]);
    }

    fn draw_dot(buf: &mut [u8], size: u32, cx: i32, cy: i32, r: i32, color: [u8; 4]) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() + dy.abs() > r + 1 {
                    continue;
                }
                let x = cx + dx;
                let y = cy + dy;
                if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
                    let o = ((y as u32 * size + x as u32) * 4) as usize;
                    buf[o..o + 4].copy_from_slice(&color);
                }
            }
        }
    }
}

#[mod_api::sunrise_mod]
impl Hooks for AtlasMod {
    fn command(&self, _host: &mut dyn ClientHost, event: &mut CommandEvent<'_>) -> Flow {
        let raw = event.command.trim();
        if raw == "map" || raw == "atlas" {
            self.open_view(ActiveView::WorldMap);
            return Flow::Cancel;
        }
        if raw == "wps" || raw == "waypoints" {
            self.open_view(ActiveView::Waypoints);
            return Flow::Cancel;
        }
        if raw == "minimap" || raw == "atlas toggle" {
            let mut st = self.state.lock().unwrap();
            st.minimap_enabled = !st.minimap_enabled;
            st.ui_rev = st.ui_rev.wrapping_add(1);
            return Flow::Cancel;
        }
        if raw == "wp clear" {
            let mut st = self.state.lock().unwrap();
            st.waypoints.clear();
            st.wp_page = 0;
            st.world_dirty = true;
            st.ui_rev = st.ui_rev.wrapping_add(1);
            return Flow::Cancel;
        }
        if raw == "wp del" {
            let mut st = self.state.lock().unwrap();
            st.waypoints.pop();
            st.world_dirty = true;
            st.ui_rev = st.ui_rev.wrapping_add(1);
            return Flow::Cancel;
        }
        if let Some(rest) = raw.strip_prefix("wp ") {
            let name = rest.trim();
            if !name.is_empty() {
                let mut st = self.state.lock().unwrap();
                let color_idx = st.wp_color_idx;
                let wp = Waypoint {
                    name: name.to_owned(),
                    x: st.player_x.floor() as i32,
                    y: st.player_y.floor() as i32,
                    z: st.player_z.floor() as i32,
                    color_idx,
                    visible: true,
                };
                st.waypoints.push(wp);
                st.world_dirty = true;
                st.ui_rev = st.ui_rev.wrapping_add(1);
            }
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        if event.action != KeyAction::Press || !host.playing() {
            return;
        }
        match event.key {
            "key.keyboard.m" => self.open_view(ActiveView::WorldMap),
            "key.keyboard.u" | "key.keyboard.b" => self.open_view(ActiveView::Waypoints),
            _ => {}
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut ClientTickPostEvent) {
        if self.open_req.swap(false, Ordering::Relaxed) {
            *self.ui.lock().unwrap() = None;
            host.mod_screen_open(ID);
        }
        if self.close_req.swap(false, Ordering::Relaxed) {
            host.mod_screen_close(ID);
        }

        if !host.playing() {
            return;
        }

        if let Some(cam) = host.camera() {
            let mut st = self.state.lock().unwrap();
            st.player_x = cam.position[0];
            st.player_y = cam.position[1];
            st.player_z = cam.position[2];
            st.tick_counter += 1;
        }

        let level = host
            .level("minecraft:overworld")
            .or_else(|| host.level("minecraft:the_nether"))
            .or_else(|| host.level("minecraft:the_end"));

        if let Some(level) = level {
            let (pcx, pcz, start_idx) = {
                let st = self.state.lock().unwrap();
                (
                    (st.player_x.floor() as i32).div_euclid(16),
                    (st.player_z.floor() as i32).div_euclid(16),
                    st.scan_ring_idx,
                )
            };

            let mut scanned = Vec::new();
            let mut idx = start_idx;
            for _ in 0..CHUNK_SCAN_BUDGET {
                let grid_i = idx % 49;
                let ox = (grid_i % 7) as i32 - 3;
                let oz = (grid_i / 7) as i32 - 3;
                let cx = pcx + ox;
                let cz = pcz + oz;
                if let Some(tile) = Self::scan_chunk(host, level, cx, cz) {
                    scanned.push(((cx, cz), tile));
                }
                idx = idx.wrapping_add(1);
            }

            let mut st = self.state.lock().unwrap();
            st.scan_ring_idx = idx;
            for (k, v) in scanned {
                st.tiles.insert(k, v);
                st.world_dirty = true;
            }
        }

        let upload_mini = {
            let mut st = self.state.lock().unwrap();
            if st.minimap_enabled && (st.tick_counter % 4 == 0 || !st.mini_created) {
                let px = st.player_x;
                let pz = st.player_z;
                let mut buf = std::mem::take(&mut st.mini_rgba);
                Self::render_buffer(
                    &st.tiles,
                    &st.waypoints,
                    px,
                    pz,
                    px,
                    pz,
                    1.0,
                    MINI_SIZE,
                    &mut buf,
                );
                st.mini_rgba = buf;
                Some((st.mini_created, st.mini_rgba.clone()))
            } else {
                None
            }
        };

        if let Some((created, pixels)) = upload_mini {
            let id = Identifier::parse(MINIMAP_TEX).unwrap();
            let res = if created {
                host.update_dynamic_texture(&id, MINI_SIZE, MINI_SIZE, &pixels)
            } else {
                host.create_dynamic_texture(&id, MINI_SIZE, MINI_SIZE, &pixels)
            };
            if res.is_ok() {
                self.state.lock().unwrap().mini_created = true;
            }
        }

        if host.mod_screen() == Some(ID) {
            let upload_world = {
                let mut st = self.state.lock().unwrap();
                if st.active_view == ActiveView::WorldMap && (st.world_dirty || !st.world_created) {
                    let vx = st.view_x;
                    let vz = st.view_z;
                    let px = st.player_x;
                    let pz = st.player_z;
                    let zoom = st.zoom;
                    let mut buf = std::mem::take(&mut st.world_rgba);
                    Self::render_buffer(
                        &st.tiles,
                        &st.waypoints,
                        vx,
                        vz,
                        px,
                        pz,
                        zoom,
                        WORLD_SIZE,
                        &mut buf,
                    );
                    st.world_rgba = buf;
                    st.world_dirty = false;
                    Some((st.world_created, st.world_rgba.clone()))
                } else {
                    None
                }
            };

            if let Some((created, pixels)) = upload_world {
                let id = Identifier::parse(WORLDMAP_TEX).unwrap();
                let res = if created {
                    host.update_dynamic_texture(&id, WORLD_SIZE, WORLD_SIZE, &pixels)
                } else {
                    host.create_dynamic_texture(&id, WORLD_SIZE, WORLD_SIZE, &pixels)
                };
                if res.is_ok() {
                    self.state.lock().unwrap().world_created = true;
                }
            }
        }
    }

    fn gui_render_post(&self, host: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
        if !host.playing() || host.mod_screen() == Some(ID) {
            return;
        }
        let (enabled, created, px, py, pz) = {
            let st = self.state.lock().unwrap();
            (
                st.minimap_enabled,
                st.mini_created,
                st.player_x.floor() as i32,
                st.player_y.floor() as i32,
                st.player_z.floor() as i32,
            )
        };
        if !enabled || !created {
            return;
        }
        let [w, _] = event.batch.size();
        let draw_size = 96.0f32;
        let x = (w - draw_size - 8.0).max(4.0);
        let y = 8.0f32;
        event
            .batch
            .fill([x - 2.0, y - 2.0, draw_size + 4.0, draw_size + 4.0], 0xCC10_1018);
        event.batch.sprite(
            MINIMAP_TEX,
            [0.0, 0.0, MINI_SIZE as f32, MINI_SIZE as f32],
            [x, y, draw_size, draw_size],
            None,
        );
        let coords = format!("{px}, {py}, {pz}");
        event.batch.text(&coords, [x, y + draw_size + 4.0], 0xFFFF_FFFF, true);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        if host.mod_screen() != Some(ID) {
            return;
        }
        let [width, height] = event.draw.size();
        event.draw.fill([0.0, 0.0, width, height], 0xEE0D_0F16);

        let (view, ui_rev, world_created, vx, vz, px, pz, zoom, wp_page, wps) = {
            let st = self.state.lock().unwrap();
            (
                st.active_view,
                st.ui_rev,
                st.world_created,
                st.view_x,
                st.view_z,
                st.player_x,
                st.player_z,
                st.zoom,
                st.wp_page,
                st.waypoints.clone(),
            )
        };

        {
            let mut ui = self.ui.lock().unwrap();
            let needs_rebuild = ui.as_ref().is_none_or(|u| {
                (u.width - width).abs() > 1.0
                    || (u.height - height).abs() > 1.0
                    || u.view != view
                    || u.ui_rev != ui_rev
            });
            if needs_rebuild {
                let st = self.state.lock().unwrap();
                *ui = Some(ScreenUi::build(width, height, &st));
            }
        }

        match view {
            ActiveView::WorldMap => {
                let map_size = (width.min(height - 48.0)).max(64.0);
                let mx = ((width - map_size) * 0.5).floor();
                let my = ((height - 34.0 - map_size) * 0.5).max(16.0).floor();

                if world_created {
                    event.draw.sprite(
                        WORLDMAP_TEX,
                        [0.0, 0.0, WORLD_SIZE as f32, WORLD_SIZE as f32],
                        [mx, my, map_size, map_size],
                        None,
                    );

                    let scale = map_size as f64 / WORLD_SIZE as f64;
                    let half = (WORLD_SIZE as f64) * 0.5;
                    for wp in &wps {
                        if !wp.visible {
                            continue;
                        }
                        let sx = mx + (((wp.x as f64 - vx) / zoom + half) * scale) as f32;
                        let sy = my + (((wp.z as f64 - vz) / zoom + half) * scale) as f32;
                        if sx >= mx && sx <= mx + map_size && sy >= my && sy <= my + map_size {
                            let (_, _, argb) = WP_COLORS[wp.color_idx % WP_COLORS.len()];
                            event.draw.text(&wp.name, [sx + 5.0, sy - 4.0], argb, true);
                        }
                    }
                }

                let header = format!(
                    "Atlas World Map  |  X: {}  Z: {}  |  Zoom: {:.1}x  |  WPs: {}",
                    vx.floor() as i32,
                    vz.floor() as i32,
                    1.0 / zoom,
                    wps.len()
                );
                event.draw.text(&header, [8.0, 5.0], 0xFFFF_FFFF, true);
            }
            ActiveView::Waypoints => {
                let total_pages = wps.len().div_ceil(WPS_PER_PAGE).max(1);
                let title = format!(
                    "Waypoints ({})  |  Page {}/{}  |  You: {}, {}",
                    wps.len(),
                    wp_page + 1,
                    total_pages,
                    px.floor() as i32,
                    pz.floor() as i32
                );
                event.draw.text(&title, [12.0, 8.0], 0xFFFF_FFFF, true);

                let ui = self.ui.lock().unwrap();
                if let Some(ScreenUi {
                    kind: ScreenUiKind::Waypoints(wui),
                    ..
                }) = ui.as_ref()
                {
                    if wui.rows.is_empty() {
                        event.draw.text(
                            "No waypoints yet. Type a name above and tap + Add Here!",
                            [12.0, 64.0],
                            0xFFAA_B4C8,
                            true,
                        );
                    } else {
                        for row in &wui.rows {
                            if let Some(wp) = wps.get(row.wp_index) {
                                let (_, _, argb) = WP_COLORS[wp.color_idx % WP_COLORS.len()];
                                event.draw.fill([12.0, row.y + 4.0, 10.0, 10.0], argb);
                                let dx = wp.x as f64 - px;
                                let dz = wp.z as f64 - pz;
                                let dist = (dx * dx + dz * dz).sqrt().round() as i32;
                                let line = format!(
                                    "{} ({}, {}, {}) - {}m",
                                    wp.name, wp.x, wp.y, wp.z, dist
                                );
                                let text_col = if wp.visible { 0xFFFF_FFFF } else { 0xFF77_7D8C };
                                event.draw.text(&line, [28.0, row.y + 5.0], text_col, true);
                            }
                        }
                    }
                }
            }
        }

        if let Some(ui) = self.ui.lock().unwrap().as_mut() {
            let _ = ui.widgets_mut().draw(&mut *event.draw, host, event.screen.mouse);
        }
    }

    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        let outcome = {
            let mut ui = self.ui.lock().unwrap();
            ui.as_mut().map(|u| {
                u.widgets_mut()
                    .mouse_pressed(host, event.screen.mouse, event.button, event.modifiers)
            })
        };

        if let Some(outcome) = outcome {
            let ui = self.ui.lock().unwrap();
            if let Some(u) = ui.as_ref() {
                match &u.kind {
                    ScreenUiKind::WorldMap(w) => match outcome {
                        Outcome::Pressed(id) if id == w.zoom_in => {
                            let mut st = self.state.lock().unwrap();
                            st.zoom = (st.zoom * 0.65).clamp(0.25, 8.0);
                            st.world_dirty = true;
                            return Flow::Cancel;
                        }
                        Outcome::Pressed(id) if id == w.zoom_out => {
                            let mut st = self.state.lock().unwrap();
                            st.zoom = (st.zoom * 1.5).clamp(0.25, 8.0);
                            st.world_dirty = true;
                            return Flow::Cancel;
                        }
                        Outcome::Pressed(id) if id == w.center => {
                            let mut st = self.state.lock().unwrap();
                            st.view_x = st.player_x;
                            st.view_z = st.player_z;
                            st.world_dirty = true;
                            return Flow::Cancel;
                        }
                        Outcome::Pressed(id) if id == w.add_wp => {
                            let mut st = self.state.lock().unwrap();
                            let n = st.waypoints.len() + 1;
                            let color_idx = st.wp_color_idx;
                            let wp = Waypoint {
                                name: format!("WP {n}"),
                                x: st.player_x.floor() as i32,
                                y: st.player_y.floor() as i32,
                                z: st.player_z.floor() as i32,
                                color_idx,
                                visible: true,
                            };
                            st.waypoints.push(wp);
                            st.world_dirty = true;
                            st.ui_rev = st.ui_rev.wrapping_add(1);
                            return Flow::Cancel;
                        }
                        Outcome::Pressed(id) if id == w.open_wps => {
                            let mut st = self.state.lock().unwrap();
                            st.active_view = ActiveView::Waypoints;
                            st.ui_rev = st.ui_rev.wrapping_add(1);
                            return Flow::Cancel;
                        }
                        Outcome::Pressed(id) if id == w.toggle_mini => {
                            let mut st = self.state.lock().unwrap();
                            st.minimap_enabled = !st.minimap_enabled;
                            st.ui_rev = st.ui_rev.wrapping_add(1);
                            return Flow::Cancel;
                        }
                        Outcome::Pressed(id) if id == w.close => {
                            self.close_req.store(true, Ordering::Relaxed);
                            return Flow::Cancel;
                        }
                        _ if outcome.handled() => return Flow::Cancel,
                        _ => {}
                    },
                    ScreenUiKind::Waypoints(w) => {
                        if let Outcome::Pressed(id) = outcome {
                            if id == w.color_btn {
                                let mut st = self.state.lock().unwrap();
                                if let Some(tf) = w.widgets.text_field(w.name_field) {
                                    st.wp_draft_name = tf.value().to_owned();
                                }
                                st.wp_color_idx = (st.wp_color_idx + 1) % WP_COLORS.len();
                                st.ui_rev = st.ui_rev.wrapping_add(1);
                                return Flow::Cancel;
                            }
                            if id == w.add_btn {
                                let mut st = self.state.lock().unwrap();
                                let raw = w
                                    .widgets
                                    .text_field(w.name_field)
                                    .map(|f| f.value().trim().to_owned())
                                    .unwrap_or_default();
                                let n = st.waypoints.len() + 1;
                                let name = if raw.is_empty() {
                                    format!("WP {n}")
                                } else {
                                    raw
                                };
                                let color_idx = st.wp_color_idx;
                                let wp = Waypoint {
                                    name,
                                    x: st.player_x.floor() as i32,
                                    y: st.player_y.floor() as i32,
                                    z: st.player_z.floor() as i32,
                                    color_idx,
                                    visible: true,
                                };
                                st.waypoints.push(wp);
                                st.wp_draft_name.clear();
                                st.world_dirty = true;
                                st.ui_rev = st.ui_rev.wrapping_add(1);
                                return Flow::Cancel;
                            }
                            if id == w.prev_btn {
                                let mut st = self.state.lock().unwrap();
                                st.wp_page = st.wp_page.saturating_sub(1);
                                st.ui_rev = st.ui_rev.wrapping_add(1);
                                return Flow::Cancel;
                            }
                            if id == w.next_btn {
                                let mut st = self.state.lock().unwrap();
                                let max_p = st.waypoints.len().div_ceil(WPS_PER_PAGE).saturating_sub(1);
                                if st.wp_page < max_p {
                                    st.wp_page += 1;
                                    st.ui_rev = st.ui_rev.wrapping_add(1);
                                }
                                return Flow::Cancel;
                            }
                            if id == w.map_btn {
                                let mut st = self.state.lock().unwrap();
                                st.active_view = ActiveView::WorldMap;
                                st.world_dirty = true;
                                st.ui_rev = st.ui_rev.wrapping_add(1);
                                return Flow::Cancel;
                            }
                            if id == w.close_btn {
                                self.close_req.store(true, Ordering::Relaxed);
                                return Flow::Cancel;
                            }
                            for row in &w.rows {
                                if id == row.goto_btn {
                                    let mut st = self.state.lock().unwrap();
                                    if let Some(wp) = st.waypoints.get(row.wp_index).cloned() {
                                        st.view_x = wp.x as f64;
                                        st.view_z = wp.z as f64;
                                        st.active_view = ActiveView::WorldMap;
                                        st.world_dirty = true;
                                        st.ui_rev = st.ui_rev.wrapping_add(1);
                                    }
                                    return Flow::Cancel;
                                }
                                if id == row.vis_btn {
                                    let mut st = self.state.lock().unwrap();
                                    if let Some(wp) = st.waypoints.get_mut(row.wp_index) {
                                        wp.visible = !wp.visible;
                                        st.world_dirty = true;
                                        st.ui_rev = st.ui_rev.wrapping_add(1);
                                    }
                                    return Flow::Cancel;
                                }
                                if id == row.del_btn {
                                    let mut st = self.state.lock().unwrap();
                                    if row.wp_index < st.waypoints.len() {
                                        st.waypoints.remove(row.wp_index);
                                        let max_p = st
                                            .waypoints
                                            .len()
                                            .div_ceil(WPS_PER_PAGE)
                                            .saturating_sub(1);
                                        if st.wp_page > max_p {
                                            st.wp_page = max_p;
                                        }
                                        st.world_dirty = true;
                                        st.ui_rev = st.ui_rev.wrapping_add(1);
                                    }
                                    return Flow::Cancel;
                                }
                            }
                        }
                        if outcome.handled() {
                            return Flow::Cancel;
                        }
                        return Flow::Cancel;
                    }
                }
            }
        }

        let mut st = self.state.lock().unwrap();
        if st.active_view == ActiveView::WorldMap {
            st.dragging = true;
            st.last_mouse = event.screen.mouse;
        }
        Flow::Cancel
    }

    fn screen_mouse_button_released_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        self.state.lock().unwrap().dragging = false;
        if let Some(ui) = self.ui.lock().unwrap().as_mut() {
            let _ = ui.widgets_mut().mouse_released(event.button);
        }
        Flow::Cancel
    }

    fn screen_mouse_dragged_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseDraggedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        let is_map = self.state.lock().unwrap().active_view == ActiveView::WorldMap;
        if is_map {
            let mut st = self.state.lock().unwrap();
            if st.dragging {
                let dx = event.screen.mouse[0] - st.last_mouse[0];
                let dy = event.screen.mouse[1] - st.last_mouse[1];
                st.last_mouse = event.screen.mouse;
                st.view_x -= (dx as f64) * st.zoom;
                st.view_z -= (dy as f64) * st.zoom;
                st.world_dirty = true;
            }
        } else if let Some(ui) = self.ui.lock().unwrap().as_mut() {
            let _ = ui
                .widgets_mut()
                .mouse_dragged(host, event.screen.mouse, event.button);
        }
        Flow::Cancel
    }

    fn screen_mouse_scrolled_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        let mut st = self.state.lock().unwrap();
        if st.active_view == ActiveView::WorldMap {
            if event.scroll[1] > 0.0 {
                st.zoom = (st.zoom * 0.8).clamp(0.25, 8.0);
            } else if event.scroll[1] < 0.0 {
                st.zoom = (st.zoom * 1.25).clamp(0.25, 8.0);
            }
            st.world_dirty = true;
        }
        Flow::Cancel
    }

    fn screen_key_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        if event.key == "key.keyboard.escape" {
            self.close_req.store(true, Ordering::Relaxed);
            return Flow::Cancel;
        }
        let view = self.state.lock().unwrap().active_view;
        if view == ActiveView::Waypoints {
            if let Some(ui) = self.ui.lock().unwrap().as_mut() {
                let out = ui
                    .widgets_mut()
                    .key_pressed(host, event.key, event.modifiers);
                if out.handled() {
                    return Flow::Cancel;
                }
            }
        } else if event.key == "key.keyboard.m" {
            self.close_req.store(true, Ordering::Relaxed);
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_char_typed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        if self.state.lock().unwrap().active_view == ActiveView::Waypoints {
            if let Some(ui) = self.ui.lock().unwrap().as_mut() {
                let out = ui.widgets_mut().char_typed(host, event.c);
                if out.handled() {
                    return Flow::Cancel;
                }
            }
        }
        Flow::Continue
    }
}
