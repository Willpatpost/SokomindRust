/** Opens one of the page's dialogs. The native <dialog> handles the rest:
 * focus trapping, Escape, the inert background, and returning focus to the
 * opener on close. */
export function openDialog(id: string): void {
  const dialog = document.getElementById(id);
  if (dialog instanceof HTMLDialogElement && !dialog.open) dialog.showModal();
}

/** Closes one of the page's dialogs, if it is open. */
export function closeDialog(id: string): void {
  const dialog = document.getElementById(id);
  if (dialog instanceof HTMLDialogElement && dialog.open) dialog.close();
}
