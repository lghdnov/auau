/**
 * Автосохранение с дебаунсом: чистая логика без Svelte и без сети.
 * Владелец состояния решает, «грязно» ли оно (getDirty) и как сохранять
 * (save); контракт save — по успеху сбросить свой флаг dirty.
 */
export interface Autosaver {
	/** Отметить изменение: перезапустить дебаунс-таймер сохранения. */
	notifyChanged(): void;
	/**
	 * Принудительное сохранение сейчас: отменяет таймер, при dirty
	 * выполняет save (сброс dirty — обязанность save). Ошибка save
	 * из flush протекает наружу — вызывающий сам решает, как её показать.
	 */
	flush(): Promise<void>;
}

/**
 * Дебаунс-автосейв: по истечении debounceMs после последнего notifyChanged
 * сохраняет, если данные всё ещё dirty. Ошибка save проглатывается и
 * логируется в console.error — данные остаются dirty, и следующий
 * notifyChanged повторит попытку.
 */
export function createAutosaver(
	getDirty: () => boolean,
	save: () => Promise<void>,
	debounceMs = 1500
): Autosaver {
	let timer: ReturnType<typeof setTimeout> | null = null;

	function cancelTimer(): void {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	}

	/** Сохранение по таймеру: ошибки не роняют приложение. */
	async function autosave(): Promise<void> {
		if (!getDirty()) return;
		try {
			await save();
		} catch (err) {
			console.error('autosave failed', err);
		}
	}

	return {
		notifyChanged(): void {
			cancelTimer();
			timer = setTimeout(() => {
				timer = null;
				void autosave();
			}, debounceMs);
		},
		async flush(): Promise<void> {
			cancelTimer();
			if (getDirty()) await save();
		}
	};
}
