<script lang="ts">
  // A model's .glb (from model_preview) in 3D: drag to turn, wheel to zoom, right-drag to move. With a cloth source,
  // "Cloth physics" runs the model's cloth live (cloth_open/cloth_step: the game's simulation, run by the app): drag
  // the model to turn or lift it and the cloth swings; "Test cloth animation" moves it through a set turn, run and
  // jump.
  import { onMount } from "svelte";
  import * as THREE from "three";
  import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
  import { OrbitControls } from "three/addons/controls/OrbitControls.js";
  import { api, message, type ClothSession, type ClothSource } from "./api";

  let {
    glb,
    loading = false,
    error = null,
    empty = "Pick a model to see it here",
    cloth = null,
  }: {
    glb: ArrayBuffer | null;
    loading?: boolean;
    error?: string | null;
    empty?: string;
    cloth?: ClothSource | null;
  } = $props();

  let host: HTMLDivElement;
  let renderer: THREE.WebGLRenderer | null = null;
  let camera: THREE.PerspectiveCamera;
  let controls: OrbitControls;
  const scene = new THREE.Scene();
  let model: THREE.Object3D | null = null;
  let grid: THREE.GridHelper | null = null;
  let home = { position: new THREE.Vector3(), target: new THREE.Vector3() };
  let parseError = $state<string | null>(null);
  let shown = $state(false);

  // ---- cloth
  /** One bone the cloth moves: its rest place in the model (and in the simulation's space), its index in the
   * session's bones, and its own rest transform to go back to. */
  interface Driven {
    bone: THREE.Object3D;
    index: number;
    restModel: THREE.Matrix4;
    restGameInverse: THREE.Matrix4;
    rest: { position: THREE.Vector3; quaternion: THREE.Quaternion; scale: THREE.Vector3 };
  }
  interface Live {
    id: number;
    driven: Driven[];
    byBone: Map<THREE.Object3D, Driven>;
    /** Where the session's bones are, in the simulation's world. */
    places: THREE.Matrix4[];
    step: number;
    testMotion: THREE.Matrix4[];
  }
  /** Changes with every model shown, so answers about an older one are dropped. */
  let token = 0;
  let clothSource: ClothSource | null = null;
  let clothAvailable = $state(false);
  let clothBusy = $state(false);
  let physics = $state(false);
  let testing = $state(false);
  let clothError = $state<string | null>(null);
  let clothParts = $state(0);
  let clothNotes = $state<string[]>([]);
  let live: Live | null = null;
  let animation = 0;
  let lastTime = 0;
  let pending = 0;
  let stepping = false;
  // where the user's hand has the model: turned by `yaw` about the vertical, `lift` above the ground; `offset` is
  // where it stands in the simulation's world (a test run moves it forward; the view keeps it in place)
  let yaw = 0;
  let lift = 0;
  let liftSpeed = 0;
  let offset = new THREE.Vector3();
  let sent = { yaw: 0, lift: 0 };
  let testFrame = 0;
  let testStart = new THREE.Matrix4();
  let drag: { id: number; x: number; y: number; t: number } | null = null;
  const raycaster = new THREE.Raycaster();

  function render() {
    renderer?.render(scene, camera);
  }

  function dispose(object: THREE.Object3D) {
    object.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (mesh.geometry) mesh.geometry.dispose();
      const materials = mesh.material ? (Array.isArray(mesh.material) ? mesh.material : [mesh.material]) : [];
      for (const m of materials) {
        for (const value of Object.values(m)) {
          if (value instanceof THREE.Texture) value.dispose();
        }
        m.dispose();
      }
    });
  }

  function clear() {
    physicsOff();
    token++;
    clothAvailable = false;
    clothBusy = false;
    clothError = null;
    if (model) {
      scene.remove(model);
      dispose(model);
      model = null;
    }
    if (grid) {
      scene.remove(grid);
      grid.dispose();
      grid = null;
    }
    shown = false;
  }

  /** Frames the model from the front, a little above. */
  function frame(object: THREE.Object3D) {
    const box = new THREE.Box3().setFromObject(object);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const radius = Math.max(size.length() / 2, 0.01);
    const distance = radius / Math.sin(THREE.MathUtils.degToRad(camera.fov / 2)) * 0.9;
    camera.near = distance / 100;
    camera.far = distance * 100;
    camera.updateProjectionMatrix();
    home.target.copy(center);
    home.position.set(center.x, center.y + radius * 0.15, center.z + distance);
    reset();
    const step = Math.pow(10, Math.floor(Math.log10(radius)));
    grid = new THREE.GridHelper(step * 40, 40, 0x888888, 0x888888);
    (grid.material as THREE.Material).opacity = 0.18;
    (grid.material as THREE.Material).transparent = true;
    grid.position.set(center.x, box.min.y, center.z);
    scene.add(grid);
  }

  export function reset() {
    camera.position.copy(home.position);
    controls.target.copy(home.target);
    controls.update();
    render();
  }

  async function show(data: ArrayBuffer) {
    parseError = null;
    try {
      const gltf = await new GLTFLoader().parseAsync(data, "");
      clear();
      model = gltf.scene;
      scene.add(model);
      frame(model);
      shown = true;
      render();
      void checkCloth();
    } catch (e) {
      clear();
      parseError = e instanceof Error ? e.message : String(e);
    }
  }

  /** Whether the model shown has cloth: the cloth buttons are offered if so. */
  async function checkCloth() {
    const mine = token;
    clothSource = cloth ? { ...cloth } : null;
    if (!clothSource) return;
    try {
      const has = await api.hasCloth(clothSource);
      if (mine === token) clothAvailable = has;
    } catch {
      // (no cloth buttons then)
    }
  }

  const matrix = (m: number[]) => new THREE.Matrix4().fromArray(m);

  /** The session's bones found in the model, parents before children. */
  function makeLive(root: THREE.Object3D, s: ClothSession): { live: Live; missing: string[] } {
    root.updateMatrixWorld(true);
    const rootInverse = root.matrixWorld.clone().invert();
    const byName = new Map<string, THREE.Object3D>();
    root.traverse((o) => {
      if (o.name && !byName.has(o.name)) byName.set(o.name, o);
      // bones move away from where the bounds were taken
      if ((o as THREE.Mesh).isMesh) o.frustumCulled = false;
    });
    const missing: string[] = [];
    const driven: Driven[] = [];
    s.bones.forEach((b, index) => {
      const bone = byName.get(THREE.PropertyBinding.sanitizeNodeName(b.name));
      if (!bone) {
        missing.push(b.name);
        return;
      }
      driven.push({
        bone,
        index,
        restModel: rootInverse.clone().multiply(bone.matrixWorld),
        restGameInverse: matrix(b.rest).invert(),
        rest: { position: bone.position.clone(), quaternion: bone.quaternion.clone(), scale: bone.scale.clone() },
      });
    });
    const depth = (o: THREE.Object3D) => {
      let n = 0;
      for (let x = o.parent; x; x = x.parent) n++;
      return n;
    };
    driven.sort((a, b) => depth(a.bone) - depth(b.bone));
    return {
      live: {
        id: s.id,
        driven,
        byBone: new Map(driven.map((d) => [d.bone, d])),
        places: s.places.map(matrix),
        step: s.step,
        testMotion: s.test_motion.map(matrix),
      },
      missing,
    };
  }

  /** The model turned by `y` about the vertical, `l` above the ground, at `x`, `z`. */
  function place(y: number, l: number, x = 0, z = 0) {
    return new THREE.Matrix4().makeRotationY(y).setPosition(x, l, z);
  }

  /** Poses the model: each bone the cloth moves where the simulation has it relative to the model (its rest place
   * moved as the simulation moved it), every other bone following its parent; the model itself at `shownAt` (the
   * simulation had it at `simAt`). */
  function pose(shownAt: THREE.Matrix4, simAt: THREE.Matrix4) {
    if (!live || !model) return;
    const root = model;
    const l = live;
    const simInverse = simAt.clone().invert();
    const posed = new Map<THREE.Object3D, THREE.Matrix4>();
    const inModel = (o: THREE.Object3D): THREE.Matrix4 => {
      if (o === root || !o.parent) return new THREE.Matrix4();
      const known = posed.get(o);
      if (known) return known;
      const d = l.byBone.get(o);
      const m = d
        ? simInverse.clone().multiply(l.places[d.index]).multiply(d.restGameInverse).multiply(d.restModel)
        : inModel(o.parent).clone().multiply(o.matrix);
      posed.set(o, m);
      return m;
    };
    for (const d of l.driven) {
      const local = inModel(d.bone.parent!).clone().invert().multiply(inModel(d.bone));
      local.decompose(d.bone.position, d.bone.quaternion, d.bone.scale);
    }
    root.matrixAutoUpdate = false;
    root.matrix.copy(shownAt);
    root.matrixWorldNeedsUpdate = true;
  }

  /** Each animation frame: the time since the last goes into steps; the model's places for them (the user's hand,
   * spread over the steps, or the test motion's) go to the simulation, and its answer poses the model. */
  function tick(now: number) {
    if (!physics || !live) return;
    const elapsed = Math.min((now - lastTime) / 1000, 0.1);
    lastTime = now;
    // let go above the ground, it falls
    if (!drag && !testing && (lift > 0 || liftSpeed > 0)) {
      liftSpeed -= 9.81 * elapsed;
      lift += liftSpeed * elapsed;
      if (lift <= 0) {
        lift = 0;
        liftSpeed = 0;
      }
    }
    pending = Math.min(pending + elapsed, live.step * 8);
    const n = Math.floor(pending / live.step);
    if (!stepping && n > 0) {
      pending -= n * live.step;
      const models: THREE.Matrix4[] = [];
      let shownAt: THREE.Matrix4;
      if (testing) {
        let last = live.testMotion[testFrame];
        for (let i = 0; i < n && testFrame + 1 < live.testMotion.length; i++) {
          testFrame++;
          last = live.testMotion[testFrame];
          models.push(testStart.clone().multiply(last));
        }
        // shown in place: turned and lifted as the test has it, not moved forward
        const inPlace = last.clone();
        inPlace.elements[12] = 0;
        inPlace.elements[14] = 0;
        shownAt = place(yaw, lift).multiply(inPlace);
        if (testFrame + 1 >= live.testMotion.length) endTest();
      } else {
        for (let i = 1; i <= n; i++) {
          const u = i / n;
          models.push(place(sent.yaw + (yaw - sent.yaw) * u, sent.lift + (lift - sent.lift) * u, offset.x, offset.z));
        }
        sent = { yaw, lift };
        shownAt = place(yaw, lift);
      }
      if (models.length) {
        const id = live.id;
        const simAt = models[models.length - 1];
        stepping = true;
        api
          .clothStep(
            id,
            models.map((m) => m.elements),
          )
          .then((buffer) => {
            if (!live || live.id !== id) return;
            const floats = new Float32Array(buffer);
            live.places.forEach((m, i) => m.fromArray(floats, i * 16));
            pose(shownAt, simAt);
            render();
          })
          .catch((e) => {
            clothError = message(e);
            physicsOff();
          })
          .finally(() => {
            stepping = false;
          });
      }
    }
    animation = requestAnimationFrame(tick);
  }

  async function physicsOn(): Promise<boolean> {
    if (!model || !clothSource) return false;
    const mine = token;
    clothBusy = true;
    clothError = null;
    try {
      const s = await api.clothOpen(clothSource);
      if (mine !== token || !model) {
        if (s) void api.clothClose(s.id);
        return false;
      }
      if (!s) {
        clothAvailable = false;
        return false;
      }
      const made = makeLive(model, s);
      live = made.live;
      clothParts = s.parts.length;
      clothNotes = [...s.warnings];
      if (made.missing.length) {
        clothNotes.push(
          `${made.missing.length} bone(s) the cloth moves aren't in this model: ${made.missing.slice(0, 5).join(", ")}`,
        );
      }
      yaw = 0;
      lift = 0;
      liftSpeed = 0;
      offset = new THREE.Vector3();
      sent = { yaw: 0, lift: 0 };
      pending = 0;
      physics = true;
      pose(new THREE.Matrix4(), new THREE.Matrix4());
      render();
      lastTime = performance.now();
      animation = requestAnimationFrame(tick);
      return true;
    } catch (e) {
      if (mine === token) clothError = message(e);
      return false;
    } finally {
      if (mine === token) clothBusy = false;
    }
  }

  function physicsOff() {
    cancelAnimationFrame(animation);
    drag = null;
    testing = false;
    physics = false;
    if (live) {
      void api.clothClose(live.id).catch(() => {});
      for (const d of live.driven) {
        d.bone.position.copy(d.rest.position);
        d.bone.quaternion.copy(d.rest.quaternion);
        d.bone.scale.copy(d.rest.scale);
      }
      live = null;
    }
    if (model) {
      model.matrix.identity();
      model.matrixAutoUpdate = true;
    }
    render();
  }

  async function togglePhysics() {
    if (physics) physicsOff();
    else await physicsOn();
  }

  /** Runs the test motion from where the model is now. */
  async function testCloth() {
    if (!physics && !(await physicsOn())) return;
    if (!live) return;
    lift = Math.max(lift, 0);
    liftSpeed = 0;
    testStart = place(yaw, lift, offset.x, offset.z);
    testFrame = 0;
    testing = true;
  }

  /** After the test motion: the model stays where it took it (turned back, forward in the simulation's world). */
  function endTest() {
    if (!live) return;
    const end = testStart.clone().multiply(live.testMotion[live.testMotion.length - 1]);
    offset.set(end.elements[12], 0, end.elements[14]);
    sent = { yaw, lift };
    testing = false;
  }

  // ---- the user's hand: with physics on, a drag that starts on the model turns it (sideways) and lifts it (up and
  // down); let go above the ground, it falls. A drag elsewhere moves the camera, as always.
  function onPointerDown(e: PointerEvent) {
    if (!physics || testing || !model || e.button !== 0) return;
    const rect = host.getBoundingClientRect();
    const at = new THREE.Vector2(
      ((e.clientX - rect.left) / rect.width) * 2 - 1,
      -((e.clientY - rect.top) / rect.height) * 2 + 1,
    );
    raycaster.setFromCamera(at, camera);
    if (raycaster.intersectObject(model, true).length === 0) return;
    // the camera doesn't get this drag
    e.stopPropagation();
    e.preventDefault();
    try {
      host.setPointerCapture(e.pointerId);
    } catch {
      // (a pointer that can't be captured still drags while it stays over the view)
    }
    drag ={ id: e.pointerId, x: e.clientX, y: e.clientY, t: performance.now() };
    liftSpeed = 0;
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.id) return;
    const now = performance.now();
    const seconds = Math.max((now - drag.t) / 1000, 0.001);
    yaw += (e.clientX - drag.x) * 0.012;
    // a pixel up is as high as a pixel is at the camera's target
    const distance = camera.position.distanceTo(controls.target);
    const perPixel = (2 * distance * Math.tan(THREE.MathUtils.degToRad(camera.fov / 2))) / Math.max(host.clientHeight, 1);
    const raised = Math.max(0, lift - (e.clientY - drag.y) * perPixel);
    liftSpeed = (raised - lift) / seconds;
    lift = raised;
    drag = { id: drag.id, x: e.clientX, y: e.clientY, t: now };
  }

  function onPointerUp(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.id) return;
    drag = null;
    if (host.hasPointerCapture(e.pointerId)) host.releasePointerCapture(e.pointerId);
    // a quick flick up throws it; on the ground it stays
    if (lift <= 0) liftSpeed = 0;
  }

  $effect(() => {
    // (glb is read first so the effect runs again when it changes, also before the renderer exists)
    const data = glb;
    if (!renderer) return;
    if (data) void show(data);
    else {
      clear();
      render();
    }
  });

  onMount(() => {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    host.appendChild(renderer.domElement);
    camera = new THREE.PerspectiveCamera(35, 1, 0.01, 1000);
    controls = new OrbitControls(camera, renderer.domElement);
    controls.addEventListener("change", render);
    scene.add(new THREE.HemisphereLight(0xffffff, 0x50555c, 2.2));
    const key = new THREE.DirectionalLight(0xffffff, 1.6);
    key.position.set(1, 2, 3);
    scene.add(key);
    const back = new THREE.DirectionalLight(0xffffff, 0.6);
    back.position.set(-2, 1, -3);
    scene.add(back);
    // (capture: before the camera controls on the canvas inside)
    host.addEventListener("pointerdown", onPointerDown, { capture: true });
    host.addEventListener("pointermove", onPointerMove);
    host.addEventListener("pointerup", onPointerUp);
    host.addEventListener("pointercancel", onPointerUp);

    const resize = () => {
      const w = Math.max(host.clientWidth, 1);
      const h = Math.max(host.clientHeight, 1);
      renderer!.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      render();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(host);
    resize();
    if (glb) void show(glb);
    return () => {
      observer.disconnect();
      host.removeEventListener("pointerdown", onPointerDown, { capture: true });
      host.removeEventListener("pointermove", onPointerMove);
      host.removeEventListener("pointerup", onPointerUp);
      host.removeEventListener("pointercancel", onPointerUp);
      clear();
      controls.dispose();
      renderer?.dispose();
      renderer = null;
    };
  });
</script>

<div class="viewer">
  <div class="canvas" class:grab={physics && !testing} bind:this={host}></div>
  {#if loading}
    <div class="overlay"><span class="spinner"></span> Loading the model…</div>
  {:else if error || parseError}
    <div class="overlay bad">{error ?? parseError}</div>
  {:else if !glb}
    <div class="overlay muted">{empty}</div>
  {/if}
  {#if shown && !loading}
    <div class="tools">
      {#if clothAvailable}
        <button
          class="ghost small"
          class:on={physics}
          onclick={togglePhysics}
          disabled={clothBusy}
          title="Simulates the model's cloth (hair, hoods, capes) as the game does: drag the model to turn it or lift it, and watch the cloth swing"
          >{clothBusy ? "Starting cloth…" : physics ? "Cloth physics: on" : "Cloth physics: off"}</button
        >
        <button
          class="ghost small"
          onclick={testCloth}
          disabled={clothBusy || testing}
          title="Moves the model through a set turn, run and jump, with the cloth simulated"
          >Test cloth animation</button
        >
      {/if}
      <button class="ghost small" onclick={reset} title="Back to the front view">Front view</button>
    </div>
    {#if clothError}
      <div class="cloth bad">{clothError}</div>
    {:else if physics}
      <div class="cloth muted" title={clothNotes.join("\n")}>
        {testing ? "Test: turn, run (in place), jump" : `${clothParts} cloth part${clothParts === 1 ? "" : "s"} simulated`}{#if clothNotes.length}&nbsp;·
          {clothNotes.length} note{clothNotes.length === 1 ? "" : "s"}{/if}
      </div>
    {/if}
    <div class="hint">
      {#if physics}
        Drag the model to turn it, up to lift it · drag beside it to look around · wheel to zoom
      {:else}
        Drag to turn · wheel to zoom · right-drag to move
      {/if}
    </div>
  {/if}
</div>

<style>
  .viewer {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 240px;
    background: var(--viewer-bg);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .canvas,
  .canvas :global(canvas) {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
  }
  .canvas.grab {
    cursor: grab;
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 24px;
    text-align: center;
    color: var(--text);
    pointer-events: none;
  }
  .overlay.muted {
    color: var(--muted);
  }
  .overlay.bad {
    color: var(--bad);
  }
  .tools {
    position: absolute;
    top: 10px;
    right: 10px;
    display: flex;
    gap: 6px;
  }
  .tools .on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .cloth {
    position: absolute;
    top: 44px;
    right: 12px;
    max-width: 60%;
    font-size: 12px;
    text-align: right;
  }
  .cloth.muted {
    color: var(--muted);
  }
  .cloth.bad {
    color: var(--bad);
  }
  .hint {
    position: absolute;
    left: 12px;
    bottom: 10px;
    font-size: 12px;
    color: var(--muted);
    pointer-events: none;
  }
</style>
