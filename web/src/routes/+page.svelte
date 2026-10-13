<script lang="ts">
	import { onMount } from 'svelte';
	import { api, ApiError } from '$lib/api';
	import type { ProjectSummary } from '$lib/types';

	let projects = $state<ProjectSummary[] | null>(null);
	let error = $state('');

	let name = $state('');
	let description = $state('');
	let creating = $state(false);

	function errorMessage(err: unknown): string {
		return err instanceof ApiError ? err.message : 'Не удалось связаться с сервером';
	}

	async function refresh() {
		error = '';
		try {
			projects = await api.listProjects();
		} catch (err) {
			error = errorMessage(err);
		}
	}

	onMount(refresh);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const trimmed = name.trim();
		if (!trimmed || creating) return;
		creating = true;
		error = '';
		try {
			await api.createProject(trimmed, description.trim() || undefined);
			name = '';
			description = '';
			projects = await api.listProjects();
		} catch (err) {
			error = errorMessage(err);
		} finally {
			creating = false;
		}
	}
</script>

<section>
	<h1>Проекты</h1>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	{#if projects === null}
		<p>Загрузка…</p>
	{:else if projects.length === 0}
		<p>Пока нет проектов — создайте первый ниже.</p>
	{:else}
		<ul class="cards">
			{#each projects as { project, counts } (project.id)}
				<li>
					<a class="card" href="/projects/{project.id}">
						<span class="name">{project.name}</span>
						{#if project.description}
							<span class="desc">{project.description}</span>
						{/if}
						<span class="counts">
							<span>В работе: {counts.new + counts.in_progress}</span>
							<span>Готово: {counts.done}</span>
						</span>
					</a>
				</li>
			{/each}
		</ul>
	{/if}

	<form class="create" onsubmit={submit}>
		<h2>Новый проект</h2>
		<label>
			Название
			<input bind:value={name} required />
		</label>
		<label>
			Описание
			<input bind:value={description} />
		</label>
		<button type="submit" disabled={creating || !name.trim()}>Создать</button>
	</form>
</section>

<style>
	.cards {
		list-style: none;
		padding: 0;
		margin: 0 0 2rem;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(16rem, 1fr));
		gap: 1rem;
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		padding: 0.9rem 1rem;
		border: 1px solid #ddd;
		border-radius: 8px;
		text-decoration: none;
		color: inherit;
	}

	.card:hover {
		border-color: #4363d8;
		background: #f5f7ff;
	}

	.name {
		font-weight: 600;
	}

	.desc {
		color: #555;
		font-size: 0.9rem;
		overflow-wrap: anywhere;
	}

	.counts {
		display: flex;
		gap: 1rem;
		margin-top: auto;
		font-size: 0.85rem;
		color: #555;
	}

	.create {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-end;
		gap: 0.75rem;
		padding-top: 0.5rem;
		border-top: 1px solid #eee;
	}

	.create h2 {
		width: 100%;
		margin: 0.5rem 0 0;
		font-size: 1.1rem;
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
		min-width: 14rem;
	}

	.error {
		color: #c62828;
	}
</style>
