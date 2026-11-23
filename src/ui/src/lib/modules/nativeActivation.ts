/**
 * Which keys a control acts on by itself.
 *
 * The browser turns these keys into a `click` on the focused control, unless
 * a key handler prevents the default. A handler further up (the tree's roving
 * navigation, the global key guard) asks here before it takes a key, so that
 * the control is activated exactly once and by nothing else.
 */

/**
 * Whether `key` (a `KeyboardEvent.key`) activates `target` natively: a button
 * on Enter and Space, a checkbox or radio button on Space.
 */
export function activatesNatively(
  target: EventTarget | null,
  key: string,
): boolean {
  if (target instanceof HTMLButtonElement) {
    return key === "Enter" || key === " ";
  }
  if (target instanceof HTMLInputElement) {
    return (
      (target.type === "checkbox" || target.type === "radio") && key === " "
    );
  }
  return false;
}
