import type { Frame, Host, RenderRequest } from "./types";
import { PreviewLoop, type LoopView } from "./previewLoop";
import { DevelopPlan, PictureBook } from "./viewport";

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

function frameFor(request: RenderRequest): Frame {
  const width = request.view.kind === "full" ? 8 : 4;
  const height = 2;
  return {
    ticket: request.ticket,
    width,
    height,
    fidelity: request.view.kind === "full" ? "exact" : "preview",
    bytes: new Uint8Array(width * height * 3),
  };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 8; i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

async function waitFor(ready: () => boolean): Promise<void> {
  const start = Date.now();
  while (!ready()) {
    if (Date.now() - start > 1000) throw new Error("timed out waiting for a develop");
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  await flush();
}

/**
 * Pan, zoom, and resize must not call the host. Opening does, once for the
 * fit proxy and once for the full frame, and a second open of the same
 * picture does not.
 */
async function testPanIssuesNoDevelop(): Promise<void> {
  const calls: RenderRequest[] = [];
  let develops = 0;
  const host = {
    async render(request: RenderRequest): Promise<Frame> {
      calls.push(request);
      return frameFor(request);
    },
  } as Host;
  const view: LoopView = { ready: true, compare: false, scene: 1, edit: 0 };
  const loop = new PreviewLoop(
    host,
    () => view,
    () => {},
    () => {},
    () => {
      develops += 1;
    },
  );

  for (const gesture of ["pan", "zoom", "resize"] as const) {
    for (let i = 0; i < 20; i += 1) loop.viewport(gesture);
  }
  assert(calls.length === 0, "panning before a picture developed");
  assert(develops === 0, "develop counter moved on pan");

  loop.now();
  await flush();
  assert(calls.length === 2, `open issued ${calls.length} develops`);
  assert(calls[0]?.view.kind === "fit", "open starts with the fit proxy");
  assert(calls[1]?.view.kind === "full", "open then renders the full frame");
  assert(
    calls.every((call) => call.view.kind !== "hundred"),
    "open used a crop develop",
  );

  for (let i = 0; i < 40; i += 1) {
    loop.viewport("pan");
    loop.viewport("zoom");
    loop.viewport("resize");
  }
  assert(calls.length === 2, `pan issued ${calls.length - 2} develops`);
  assert(develops === calls.length, "counter does not match host calls");

  loop.now();
  await flush();
  assert(calls.length === 2, "the same picture developed again");

  view.scene = 2;
  loop.now();
  await flush();
  assert(calls.length === 4, "a new file should develop again");
  assert(calls[3]?.view.kind === "full", "the new file renders full");
}

/** A slider drag is the fast proxy. Panning during that drag still does not develop. */
async function testDragPanWaitsForSettle(): Promise<void> {
  const calls: RenderRequest[] = [];
  const host = {
    async render(request: RenderRequest): Promise<Frame> {
      calls.push(request);
      return frameFor(request);
    },
  } as Host;
  const view: LoopView = { ready: true, compare: false, scene: 1, edit: 0 };
  const loop = new PreviewLoop(
    host,
    () => view,
    () => {},
    () => {},
  );
  view.edit = 1;
  loop.drag({ control: "exposure", value: 0.25 });
  assert(calls.length === 1, "a drag should send one proxy");
  assert(calls[0]?.view.kind === "fit" && calls[0].view.longEdge === 720, "drag is the 720 proxy");
  for (let i = 0; i < 15; i += 1) loop.viewport("pan");
  assert(calls.length === 1, "panning during a drag developed");
  await waitFor(() => calls.length >= 3);
  assert(calls[1]?.view.kind === "fit" && calls[1].view.longEdge === 1440, "settle is the 1440 proxy");
  assert(calls[2]?.view.kind === "full", "settle renders the full frame");
  const settled = calls.length;
  for (let i = 0; i < 10; i += 1) loop.viewport("pan");
  assert(calls.length === settled, "panning after settle developed");
}

/** A fit reply for an edit the user has already left must not start a full frame. */
async function testStaleFitDoesNotStartFull(): Promise<void> {
  const calls: RenderRequest[] = [];
  const held: { finish: ((frame: Frame) => void) | null } = { finish: null };
  const host = {
    async render(request: RenderRequest): Promise<Frame> {
      calls.push(request);
      return new Promise((resolve) => {
        held.finish = resolve;
      });
    },
  } as Host;
  const view: LoopView = { ready: true, compare: false, scene: 1, edit: 0 };
  const loop = new PreviewLoop(
    host,
    () => view,
    () => {},
    () => {},
  );
  loop.now();
  assert(calls.length === 1, "fit did not start");
  const finish = held.finish;
  if (finish === null) throw new Error("fit was not held");
  view.edit = 3;
  finish(frameFor(calls[0]!));
  await flush();
  assert(calls.length === 1, `stale fit started a full frame (${calls.length})`);
  loop.dispose();
}

function testPairHoldsAtMostBeforeAndAfter(): void {
  const live = new Set<string>();
  const book = new PictureBook<string>();
  const release = (image: string) => {
    if (!live.delete(image)) throw new Error(`released ${image} twice`);
  };
  const putFull = (edit: number, image: string) => {
    live.add(image);
    book.putFull("after", 1, edit, image, release);
  };
  putFull(0, "full-0");
  assert(live.size === 1, "the unedited frame was stored twice");
  assert(book.canShow("before", 1, 0), "before should share the unedited frame");
  assert(!DevelopPlan.runs(DevelopPlan.compare(true)), "cached A/B still develops");
  book.invalidateEdit(release);
  assert(live.has("full-0"), "invalidating the edit dropped the before frame");
  assert(!book.canShow("after", 1, 1), "the edited side still counts as current");
  putFull(1, "full-1");
  assert(live.size === 2, `expected the A/B pair, found ${[...live].join(",")}`);
  putFull(2, "full-2");
  assert(!live.has("full-1"), "the previous after frame leaked");
  assert(live.size === 2, `a third full frame is resident: ${[...live].join(",")}`);
  const picked = book.pick("pixel", "after", 1, 2);
  assert(picked?.image === "full-2" && picked.full, "100% should show the current full frame");
  const before = book.pick("pixel", "before", 1, 2);
  assert(before?.image === "full-0", "before should stay on the unedited frame");
  book.invalidateScene(release);
  assert(live.size === 0, "closing the file leaked a frame");
}

function testCompareAndViewportPlans(): void {
  const pan = DevelopPlan.for("pan");
  const zoom = DevelopPlan.for("zoom");
  const resize = DevelopPlan.for("resize");
  assert(!DevelopPlan.runs(pan) && !DevelopPlan.runs(zoom) && !DevelopPlan.runs(resize), "viewport work");
  const edit = DevelopPlan.for("edit");
  assert(edit.fit === 720 && edit.full === false, "an edit should stay on the fast proxy");
  const missing = DevelopPlan.compare(false);
  assert(missing.fit === 1440 && missing.full, "a missing A/B side should develop");
}

export async function runViewportTests(): Promise<void> {
  testCompareAndViewportPlans();
  testPairHoldsAtMostBeforeAndAfter();
  await testPanIssuesNoDevelop();
  await testDragPanWaitsForSettle();
  await testStaleFitDoesNotStartFull();
}
