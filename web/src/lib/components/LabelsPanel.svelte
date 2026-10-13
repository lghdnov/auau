<script lang="ts">
	import type { LabelPatch } from '$lib/api';
	import type { Label } from '$lib/types';

	interface Props {
		labels: Label[];
		/** Создать метку; hotkey — строка, '' = не задан. true = успех. */
		onCreate: (name: string, hotkey: string) => Promise<boolean>;
		/** Частично обновить метку; patch содержит только реально изменённые поля. */
		onUpdate: (id: string, patch: LabelPatch) => Promise<boolean>;
		/** Заархивировать метку; confirm уже получен. */
		onArchive: (label: Label) => Promise<boolean>;
	}

	let { labels, onCreate, onUpdate, onArchive }: Props = $props();

	let newName = $state('');
	let newHotkey = $state('');
	let creating = $state(false);

	let editingId = $state<string | null>(null);
	let editName = $state('');
	let editColor = $state('#000000');
	let editHotkey = $state('');
	let saving = $state(false);

	async function submitCreate(e: SubmitEvent) {
		e.preventDefault();
		const name = newName.trim();
		if (!name || creating) return;
		creating = true;
		try {
			const ok = await onCreate(name, newHotkey.trim());
			if (ok) {
				newName = '';
				newHotkey = '';
			}
		} finally {
			creating = false;
		}
	}

	function startEdit(label: Label) {
		editingId = label.id;
		editName = label.name;
		editColor = label.color;
		editHotkey = label.hotkey ?? '';
	}

	async function submitEdit(e: SubmitEvent) {
		e.preventDefault();
		const label = labels.find((l) => l.id === editingId);
		if (!label || saving) return;

		const patch: LabelPatch = {};
		const name = editName.trim();
		if (name && name !== label.name) patch.name = name;
		if (editColor !== label.color) patch.color = editColor;
		const hotkey = editHotkey.trim();
		if (hotkey !== (label.hotkey ?? '') && hotkey) patch.hotkey = hotkey;

		if (Object.keys(patch).length === 0) {
			editingId = null;
			return;
		}
		saving = true;
		try {
			const ok = await onUpdate(label.id, patch);
			if (ok) editingId = null;
		} finally {
			saving = false;
		}
	}

	async function archive(label: Label) {
		if (!confirm(`Заархивировать метку «${label.name}»?`)) return;
		await onArchive(label);
	}
</script>

<ul class="labels">
	{#each labels as label (label.id)}
		<li class:archived={label.archived}>
			<div class="row">
				<span class="swatch" style:background={label.color} aria-hidden="true"></span>
				<span class="name">{label.name}</span>
				{#if label.hotkey}
					<kbd>{label.hotkey}</kbd>
				{/if}
				{#if label.archived}
					<span class="badge">в архиве</span>
				{:else}
					<span class="actions">
						<button type="button" onclick={() => startEdit(label)}>Изменить</button>
						<button type="button" class="danger" onclick={() => archive(label)}>
							В архив
						</button>
					</span>
				{/if}
			</div>

			{#if editingId === label.id}
				<form class="edit" onsubmit={submitEdit}>
					<label>
						Название
						<input bind:value={editName} required />
					</label>
					<label>
						Цвет
						<input type="color" bind:value={editColor} />
					</label>
					<label>
						Хоткей
						<input bind:value={editHotkey} placeholder="например, s" />
					</label>
					<span class="hint">Хоткей можно заменить, но нельзя очистить.</span>
					<span class="actions">
						<button type="submit" disabled={saving}>Сохранить</button>
						<button type="button" onclick={() => (editingId = null)}>Отмена</button>
					</span>
				</form>
			{/if}
		</li>
	{:else}
		<li class="empty">Метки не заданы — создайте первую.</li>
	{/each}
</ul>

<form class="create" onsubmit={submitCreate}>
	<label>
		Название
		<input bind:value={newName} required />
	</label>
	<label>
		Хоткей (необязательно)
		<input bind:value={newHotkey} placeholder="например, d" />
	</label>
	<button type="submit" disabled={creating || !newName.trim()}>Добавить метку</button>
</form>

<style>
	.labels {
		list-style: none;
		padding: 0;
		margin: 0 0 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	li {
		border-bottom: 1px solid #eee;
		padding: 0.35rem 0;
	}

	.row {
		display: flex;
		align-items: center;
		gap: 0.6rem;
	}

	.swatch {
		width: 1rem;
		height: 1rem;
		border-radius: 3px;
		flex-shrink: 0;
	}

	.name {
		overflow-wrap: anywhere;
	}

	kbd {
		border: 1px solid #ccc;
		border-radius: 4px;
		padding: 0 0.35rem;
		font-size: 0.8rem;
		background: #f7f7f7;
	}

	.badge {
		color: #777;
		font-size: 0.8rem;
	}

	.archived {
		opacity: 0.55;
	}

	.actions {
		margin-left: auto;
		display: inline-flex;
		gap: 0.4rem;
	}

	.danger {
		color: #c62828;
	}

	.edit,
	.create {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-end;
		gap: 0.75rem;
		margin-top: 0.5rem;
	}

	.edit {
		padding: 0.5rem 0 0.25rem 1.6rem;
		border-left: 2px solid #e5e5e5;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		font-size: 0.85rem;
		color: #555;
	}

	input {
		padding: 0.3rem 0.5rem;
		border: 1px solid #ccc;
		border-radius: 6px;
	}

	input[type='color'] {
		padding: 0.1rem;
		width: 3rem;
		height: 1.9rem;
	}

	.hint {
		font-size: 0.8rem;
		color: #777;
		align-self: center;
	}

	.empty {
		color: #555;
	}
</style>
