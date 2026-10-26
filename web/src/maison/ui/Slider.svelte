<script>
  /**
   * A range slider that stays smooth under the finger: while it is held, nothing
   * reactive changes (the fill is painted straight on the element), and the
   * value is only reported on release. A new `value` from outside is applied
   * once the slider is let go.
   *
   * tone: « level » fills up to the thumb; « kelvin » shows warm to cold whites.
   */
  let { min, max, step = 1, value, label, tone = 'level', onlive = null, oncommit } = $props();

  let el = $state();
  let held = false;

  const paint = () => el?.style.setProperty('--p', `${((Number(el.value) - min) / (max - min)) * 100}%`);

  $effect(() => {
    if (el && !held) {
      el.value = String(value);
      paint();
    }
  });

  function live() {
    paint();
    onlive?.(Number(el.value));
  }

  function commit() {
    held = false;
    oncommit(Number(el.value));
  }
</script>

<input
  bind:this={el}
  class="slider {tone}"
  type="range"
  {min}
  {max}
  {step}
  aria-label={label}
  onpointerdown={() => (held = true)}
  onpointercancel={() => (held = false)}
  oninput={live}
  onchange={commit} />

<style>
  .slider {
    appearance: none;
    -webkit-appearance: none;
    flex: 1;
    min-width: 0;
    width: 100%;
    height: 6px;
    margin: 14px 0;
    border-radius: 999px;
    background: linear-gradient(
      to right,
      color-mix(in srgb, var(--warm) 65%, #fff),
      var(--warm) var(--p, 50%),
      var(--surface-3) var(--p, 50%)
    );
    box-shadow: inset 0 1px 2px rgb(0 0 0 / 12%);
    outline: none;
    cursor: pointer;
    touch-action: pan-y;
  }

  .slider.kelvin {
    background: linear-gradient(90deg, #ffb35c, #ffe9d2 45%, #e9f0ff 70%, #bcd2ff);
  }

  /* A small thumb that swells when it is held, hovered or focused. */
  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    border: 0;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 4px rgb(0 0 0 / 35%), 0 0 0 1px rgb(0 0 0 / 8%);
    transition: transform 0.15s var(--ease);
  }

  .slider::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border: 0;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 4px rgb(0 0 0 / 35%), 0 0 0 1px rgb(0 0 0 / 8%);
    transition: transform 0.15s var(--ease);
  }

  .slider:hover::-webkit-slider-thumb,
  .slider:focus-visible::-webkit-slider-thumb,
  .slider:active::-webkit-slider-thumb {
    transform: scale(1.7);
  }

  .slider:hover::-moz-range-thumb,
  .slider:focus-visible::-moz-range-thumb,
  .slider:active::-moz-range-thumb {
    transform: scale(1.7);
  }

  .slider::-moz-range-track {
    background: transparent;
  }

  .slider:focus-visible {
    outline: 2px solid var(--cool);
    outline-offset: 6px;
  }

  @media (prefers-reduced-motion: reduce) {
    .slider::-webkit-slider-thumb,
    .slider::-moz-range-thumb {
      transition: none;
    }
  }
</style>
