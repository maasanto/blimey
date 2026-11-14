// A pixel cue's layers as the stage composer edits them (docs/api.md, Pixel cues).

export const FULL_STAGE = { x: 0, y: 0, width: 1, height: 1 };
export const MAX_LAYERS = 8;
/** The smallest area a resize leaves, in canvas units. */
const MIN_SIZE = 0.025;
/** An edge this close to a fixture's edge, in canvas units, lands on it. */
const SNAP_DISTANCE = 0.015;
/** Elsewhere an edge lands on steps of a fortieth of the canvas. */
const GRID_STEP = 0.025;

/** A beat's punch, as a groove plays it. */
const PUNCH = { shape: 'ramp-down', length: '0.5 beats', level: '100%' };

/** Plan §16's example swell. */
const SWELL = { duration: '4 bars', curve: 'ease-in' };

/** The inspector's grooves for a bar of `barBeats` beats: each a layer's `pattern` or `swell`. */
export function grooves(barBeats) {
  const onBar = (beats) => beats.filter((at) => at - 1 < barBeats).map((at) => ({ at, ...PUNCH }));
  const beats = Array.from({ length: Math.ceil(barBeats) }, (_, index) => index + 1);
  const loop = (hits) => ({ pattern: { length: '1 bar', hits } });
  return {
    'on the beat': loop(onBar(beats)),
    'off-beat': loop(onBar(beats.map((beat) => beat + 0.5))),
    heartbeat: loop([{ at: 1, ...PUNCH }, { at: 1.5, ...PUNCH, level: '60%' }]),
    backbeat: loop(onBar(beats.filter((beat) => beat % 2 === 0))),
    swell: { swell: SWELL },
  };
}

/** What each ingredient of the tray starts as, besides its colour; new areas cover what is below. */
const RECIPES = {
  Wash: { shape: 'fill' },
  Chase: { shape: 'bar', effect: 'chase', params: { width: 0.25, speed: '1 bar' } },
  Sweep: { shape: 'sweep', effect: 'move', params: { speed: '1 bar' } },
  Sparkle: { shape: 'fill', effect: 'sparkle', params: { width: 0.4, speed: '2 beats' } },
  Breathe: { shape: 'fill', effect: 'pulse', params: { speed: '2 bars' } },
  Swell: { shape: 'fill', groove: 'swell' },
  Hits: { shape: 'fill', groove: 'on the beat' },
};
export const INGREDIENTS = Object.keys(RECIPES);

/**
 * A new layer of `ingredient` in `color`, on `area` or, without one, the whole canvas, its
 * groove in bars of `barBeats` beats.
 */
export function newLayer(ingredient, color, area, barBeats) {
  const { params, groove, ...recipe } = RECIPES[ingredient];
  const rhythm = groove && grooves(barBeats)[groove];
  return { ...(area && { area }), ...recipe, params: { color, ...params }, ...rhythm, blend: 'over' };
}

/** The cue with `layers`: one in the older form becomes the same layers over the whole canvas. */
export function asLayers(cue) {
  if (cue.layers) return cue;
  const { shape, effect, params, layer_2, ...rest } = cue;
  const first = { ...(shape && { shape }), ...(effect && { effect }), ...(params && { params }) };
  return { ...rest, layers: layer_2 ? [first, layer_2] : [first] };
}

/**
 * A scene cue as a pixel cue (plan §16): a whole-stage `fill` per colour and dimmer the scene
 * gives, muting the fixtures and grids that colour doesn't reach. A fixture given only a dimmer
 * goes dark, as it does fired alone: a pixel cue has no colour from below to keep.
 */
export function sceneAsPixelCue({ type, values = {}, ...rest }, rig) {
  const looks = new Map();
  for (const [target, settings] of Object.entries(values)) {
    for (const name of rig.groups[target] ?? [target]) looks.set(name, { ...looks.get(name), ...settings });
  }
  const recipes = new Map();
  for (const [name, { color, dimmer = '100%' }] of looks) {
    const isDark = !color || parseFloat(dimmer) === 0;
    const level = isDark || parseFloat(dimmer) === 100 ? {} : { level: dimmer };
    const layer = { shape: 'fill', params: { color: isDark ? '#000000' : color }, ...level, blend: 'over' };
    const key = JSON.stringify(layer);
    recipes.set(key, { layer, fixtures: [...(recipes.get(key)?.fixtures ?? []), name] });
  }
  const everything = [...rig.fixtures, ...(rig.grids ?? [])].map(({ name }) => name);
  const layers = [...recipes.values()].map(({ layer, fixtures }) => {
    const mute = muteList(everything.filter((name) => !fixtures.includes(name)), rig.groups);
    return mute.length ? { ...layer, mute } : layer;
  });
  // ponytail: one rgbw for the whole cue, the first given; a scene mixing modes keeps only that one.
  const rgbw = [...looks.values()].find((look) => look.rgbw)?.rgbw;
  return { ...rest, type: 'pixel', ...(rgbw && { rgbw }), layers };
}

/** `names` as a `mute` list, whole groups by their name, the largest first. */
function muteList(names, groups) {
  const left = new Set(names);
  const mute = [];
  for (const [group, members] of Object.entries(groups).sort(([, a], [, b]) => b.length - a.length)) {
    if (!members.length || !members.every((member) => left.has(member))) continue;
    mute.push(group);
    members.forEach((member) => left.delete(member));
  }
  return [...mute, ...left];
}

export const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
export const tidy = (value) => Math.round(value * 1000) / 1000;

/** How far each field of `area` can go, the others held, with the area still on the canvas. */
const fieldLimits = (area) => ({
  x: [0, 1 - area.width],
  y: [0, 1 - area.height],
  width: [MIN_SIZE, 1 - area.x],
  height: [MIN_SIZE, 1 - area.y],
});

/** `area` with `key` set to `value`, limited so the area stays on the canvas. */
export const withAreaField = (area, key, value) => ({ ...area, [key]: tidy(clamp(value, ...fieldLimits(area)[key])) });

export const isSameArea = (area, other) => ['x', 'y', 'width', 'height'].every((key) => area[key] === other[key]);

/** As the engine draws: left and bottom edges in, right and top ones only on the canvas's edge. */
function within(position, start, length) {
  const end = start + length;
  return position >= start && (position < end || (end >= 1 && position <= end));
}

export const catches = (area, { x, y }) => within(x, area.x, area.width) && within(y, area.y, area.height);

/** Each fixture and grid with a pixel in `area`: name → the set of those pixels' indices. */
export function caughtPixels(rig, area) {
  const caught = new Map();
  for (const { name, pixels } of [...rig.fixtures, ...(rig.grids ?? [])]) {
    const indices = pixels.flatMap((pixel, index) => (catches(area, pixel) ? [index] : []));
    if (indices.length) caught.set(name, new Set(indices));
  }
  return caught;
}

/** The `mute` entry that leaves the fixture or grid `name` alone on `layer`: its name or a group. */
export const mutedBy = (layer, groups, name) =>
  layer.mute?.find((muted) => muted === name || groups[muted]?.includes(name));

function smallestGap(positions) {
  const sorted = [...new Set(positions)].sort((a, b) => a - b);
  return Math.min(...sorted.slice(1).map((position, index) => position - sorted[index]));
}

/**
 * The edges areas snap to, in canvas units: the canvas's, and each fixture's and grid's, its
 * pixels padded by half their spacing, so an area on an edge holds the fixture or leaves it whole.
 */
export function stageEdges(rig) {
  const edges = { x: [0, 1], y: [0, 1] };
  for (const { pixels } of [...rig.fixtures, ...(rig.grids ?? [])]) {
    const xs = pixels.map((pixel) => pixel.x);
    const ys = pixels.map((pixel) => pixel.y);
    // A single row or column takes its spacing from the other way.
    const spacing = [smallestGap(xs), smallestGap(ys)].filter(Number.isFinite);
    const pad = (gap) => (Number.isFinite(gap) ? gap : Math.min(...spacing, GRID_STEP)) / 2;
    const [padX, padY] = [pad(smallestGap(xs)), pad(smallestGap(ys))];
    edges.x.push(tidy(Math.min(...xs) - padX), tidy(Math.max(...xs) + padX));
    edges.y.push(tidy(Math.min(...ys) - padY), tidy(Math.max(...ys) + padY));
  }
  return edges;
}

/** Where an edge dragged to `value` lands: on the nearest of `lines` if close, else on the grid. */
function snap(value, lines) {
  const nearest = lines.reduce((best, line) => (Math.abs(line - value) < Math.abs(best - value) ? line : best), Infinity);
  return Math.abs(nearest - value) <= SNAP_DISTANCE ? nearest : tidy(Math.round(value / GRID_STEP) * GRID_STEP);
}

/** `area` moved by (`dx`, `dy`), whichever of its edges lands nearer a line taking it there. */
export function moveArea(area, dx, dy, edges) {
  const along = (start, length, delta, lines) => {
    const wanted = start + delta;
    const byStart = snap(wanted, lines);
    const byEnd = snap(wanted + length, lines) - length;
    return Math.abs(byEnd - wanted) < Math.abs(byStart - wanted) ? byEnd : byStart;
  };
  const moved = withAreaField(area, 'x', along(area.x, area.width, dx, edges.x));
  return withAreaField(moved, 'y', along(area.y, area.height, dy, edges.y));
}

/** `area` with the edges in `sides` (`left`, `right`, `top`, `bottom`) dragged by (`dx`, `dy`). */
export function resizeArea(area, sides, dx, dy, edges) {
  let [left, right, bottom, top] = [area.x, area.x + area.width, area.y, area.y + area.height];
  if (sides.includes('left')) left = clamp(snap(left + dx, edges.x), 0, right - MIN_SIZE);
  if (sides.includes('right')) right = clamp(snap(right + dx, edges.x), left + MIN_SIZE, 1);
  if (sides.includes('bottom')) bottom = clamp(snap(bottom + dy, edges.y), 0, top - MIN_SIZE);
  if (sides.includes('top')) top = clamp(snap(top + dy, edges.y), bottom + MIN_SIZE, 1);
  return { x: tidy(left), y: tidy(bottom), width: tidy(right - left), height: tidy(top - bottom) };
}

/** What a layer is, in a word, from its shape and effect. */
export function kindOf(layer) {
  const shape = layer.shape ?? 'bar';
  const effect = layer.effect ?? 'static';
  if (effect === 'pulse') return 'breathe';
  if (effect === 'move') return shape === 'sweep' ? 'sweep' : `moving ${shape}`;
  if (effect !== 'static') return effect;
  return shape === 'fill' ? 'wash' : shape;
}

/** Where an area sits, as the audience sees it. */
export function whereOf(area = FULL_STAGE) {
  const isWide = area.width >= 0.9;
  const isTall = area.height >= 0.9;
  const centreX = area.x + area.width / 2;
  const centreY = area.y + area.height / 2;
  const side = centreX < 0.4 ? 'left' : centreX > 0.6 ? 'right' : 'centre';
  const height = centreY > 0.6 ? 'top' : centreY < 0.4 ? 'bottom' : 'middle';
  if (isWide && isTall) return 'everywhere';
  if (isTall) return side === 'centre' ? 'in the centre' : `on the ${side}`;
  if (isWide) return `across the ${height}`;
  return side === 'centre' && height === 'middle' ? 'in the middle' : `at the ${height} ${side}`;
}

/** One layer read back: "amber chase on the right". */
export function describeLayer(layer) {
  const muted = layer.mute?.length ? ` except ${layer.mute.join(' and ')}` : '';
  return `${layer.params?.color ?? 'white'} ${kindOf(layer)} ${whereOf(layer.area)}${muted}`;
}

/** The look read back in one sentence, bottom layer first. */
export function describeLook(layers) {
  if (!layers.length) return 'Nothing on stage yet.';
  const text = layers.map(describeLayer).join(', ');
  return `${text[0].toUpperCase()}${text.slice(1)}.`;
}
