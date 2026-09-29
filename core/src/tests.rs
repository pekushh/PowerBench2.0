//! Критерии приёмки Этапа 1 и общая сериализация тестов.
//!
//! Общий мьютекс `lock()` удерживается всеми тестами крейта: тест «0 аллокаций»
//! включает глобальный считающий аллокатор, и параллельные тесты не должны
//! влиять на его счёт. Поэтому каждая тест-функция крейта вызывает `lock()`.

use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::checksum::{mix, start_run_checksum};
use crate::config::{Phase, Profile};
use crate::engine::{Engine, RunError, RunTarget};

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Сериализация тела теста (см. доккомментарий модуля).
pub fn lock() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap()
}

/// (а) Два прогона по N тиков дают одинаковый runChecksum — для всех фаз.
#[test]
fn two_runs_of_n_ticks_have_identical_checksum() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    for phase in [Phase::Light, Phase::Heavy, Phase::Response] {
        let ticks = if phase == Phase::Response { 256u64 } else { 64 };
        let target = RunTarget::Ticks(ticks);
        engine.reset();
        let first = engine.run_phase(phase, target).unwrap();
        engine.reset();
        let second = engine.run_phase(phase, target).unwrap();
        assert_eq!(
            first.run_checksum, second.run_checksum,
            "runChecksum различается между прогонами фазы {phase:?}"
        );
        assert_eq!(
            first.first_tick_checksum, second.first_tick_checksum,
            "первый тик фазы {phase:?} не детерминирован"
        );
        assert_eq!(first.ticks, ticks);
    }
}

/// (б) Reset восстанавливает first-tick checksum и контрольную сумму запуска.
#[test]
fn reset_restores_first_tick_checksum() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));

    engine.reset();
    let a = engine.run_phase(Phase::Light, RunTarget::Ticks(1)).unwrap();
    engine.reset();
    let b = engine.run_phase(Phase::Light, RunTarget::Ticks(1)).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.first_tick_checksum, b.first_tick_checksum);

    // Цепочка: после одного тика runChecksum = Mix(инициализация, tickChecksum).
    assert_eq!(
        a.run_checksum,
        mix(start_run_checksum(), a.first_tick_checksum),
        "контрольная сумма запуска после 1 тика не соответствует цепочке Mix"
    );

    // Прогон другой длины начинается с той же контрольной суммы тика.
    engine.reset();
    let c = engine.run_phase(Phase::Light, RunTarget::Ticks(8)).unwrap();
    assert_eq!(a.first_tick_checksum, c.first_tick_checksum);

    // И в другой фазе первый тик после reset снова равен эталону.
    engine.reset();
    let d = engine.run_phase(Phase::Heavy, RunTarget::Ticks(1)).unwrap();
    engine.reset();
    let e = engine.run_phase(Phase::Heavy, RunTarget::Ticks(1)).unwrap();
    assert_eq!(d, e);
}

/// (в) расписание суперцикла — покрыто в `config`; здесь дублируется требование
/// о том, что класс тика не зависит от номера раунда (смещения по 256).
#[test]
fn response_schedule_is_round_independent() {
    let _g = lock();
    use crate::config::response_profile;
    for k in 0..3u64 {
        assert_eq!(response_profile(256 * k), Profile::ResponseBase);
        assert_eq!(response_profile(256 * k + 63), Profile::ResponseMedium);
        assert_eq!(response_profile(256 * k + 127), Profile::ResponseMedium);
        assert_eq!(response_profile(256 * k + 191), Profile::ResponseMedium);
        assert_eq!(response_profile(256 * k + 255), Profile::ResponseMajor);
    }
}

/// (г) Checksum не зависит от числа воркеров.
#[test]
fn checksum_is_independent_of_worker_count() {
    let _g = lock();
    let mut reference: Option<(crate::engine::RunReport, crate::engine::RunReport)> = None;
    for workers in [1usize, 2, 3, 6, 8] {
        let mut engine = Engine::new(Some(workers));
        engine.reset();
        let heavy = engine
            .run_phase(Phase::Heavy, RunTarget::Ticks(64))
            .unwrap();
        engine.reset();
        let response = engine
            .run_phase(Phase::Response, RunTarget::Ticks(256))
            .unwrap();
        match &mut reference {
            None => reference = Some((heavy, response)),
            Some((ref_heavy, ref_response)) => {
                assert_eq!(
                    ref_heavy.run_checksum, heavy.run_checksum,
                    "Heavy: runChecksum зависит от числа воркеров ({workers})"
                );
                assert_eq!(
                    ref_heavy.first_tick_checksum, heavy.first_tick_checksum,
                    "Heavy: первый тик зависит от числа воркеров ({workers})"
                );
                assert_eq!(
                    ref_response.run_checksum, response.run_checksum,
                    "Response: runChecksum зависит от числа воркеров ({workers})"
                );
                assert_eq!(
                    ref_response.first_tick_checksum, response.first_tick_checksum,
                    "Response: первый тик зависит от числа воркеров ({workers})"
                );
            }
        }
    }
}

/// (д) В главном цикле тиков — 0 аллокаций (считающий глобальный аллокатор).
#[test]
fn zero_allocations_in_tick_loop() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    engine.reset();
    engine.prepare_sample_buffer(Phase::Heavy, 1);

    // Прогрев: ленивые инициализации (TLS, системные примитивы) должны пройти
    // до включения счётчика.
    engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(16))
        .unwrap();

    crate::alloc_count::COUNT.store(0, Ordering::Relaxed);
    crate::alloc_count::ENABLED.store(true, Ordering::Relaxed);
    let result = engine.run_phase(Phase::Heavy, RunTarget::Ticks(64));
    let counted = crate::alloc_count::COUNT.load(Ordering::Relaxed);
    crate::alloc_count::ENABLED.store(false, Ordering::Relaxed);

    assert!(result.is_ok(), "прогон не завершился: {:?}", result.err());
    assert_eq!(
        counted, 0,
        "обнаружены {counted} аллокаций в главном цикле тиков"
    );
}

/// (е) Отмена завершает фазу за ограниченное время, пул снова работает.
#[test]
fn cancellation_is_bounded_and_pool_recovers() {
    let _g = lock();
    let mut engine = Engine::new(Some(3));
    engine.reset();
    let canceller = engine.canceller();

    let (result, cancel_latency) = std::thread::scope(|s| {
        let handled = s.spawn(|| {
            engine.prepare_sample_buffer(Phase::Heavy, 60);
            engine.run_phase(Phase::Heavy, RunTarget::Duration(Duration::from_secs(60)))
        });
        // Даём прогону стартовать, затем отменяем.
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        canceller.store(true, Ordering::Release);
        let result = handled.join().unwrap();
        (result, start.elapsed())
    });

    assert!(
        matches!(result, Err(RunError::Cancelled)),
        "отмена не вернула Cancelled: {result:?}"
    );
    assert!(
        cancel_latency < Duration::from_secs(5),
        "отмена превысила ограничение по времени: {cancel_latency:?}"
    );

    // Пул восстановлен: reset + новый прогон дают корректные контрольные суммы.
    engine.reset();
    let a = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(64))
        .unwrap();
    engine.reset();
    let b = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(64))
        .unwrap();
    assert_eq!(a.run_checksum, b.run_checksum);
}

/// Регресс: после отмены пул обязан быть «тихим» перед reset, иначе воркер,
/// не уложившийся в грейс, писал бы в буферы сущностей параллельно с
/// `EntityBuffers::reset()` из главного потока (гонка памяти и рассинхрон
/// контрольных сумм). Проверяем инвариант напрямую и через reset.
#[test]
fn reset_waits_for_pool_quiescence() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    engine.reset();
    let canceller = engine.canceller();

    // Отменяем длинную фазу и дожидаемся её выхода.
    let result = std::thread::scope(|s| {
        let handled = s.spawn(|| {
            engine.prepare_sample_buffer(Phase::Heavy, 60);
            engine.run_phase(Phase::Heavy, RunTarget::Duration(Duration::from_secs(60)))
        });
        std::thread::sleep(Duration::from_millis(200));
        canceller.store(true, Ordering::Release);
        handled.join().unwrap()
    });
    assert!(matches!(result, Err(RunError::Cancelled)));

    // `reset` сам дожидается тишины — после него пул обязан быть quiesced.
    engine.reset();
    assert!(
        engine.pool_quiesced(),
        "пул не затих после reset: воркеры могли писать в буферы"
    );

    // И последующие прогоны детерминированы (нет остаточных записей).
    engine.reset();
    let a = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(64))
        .unwrap();
    engine.reset();
    let b = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(64))
        .unwrap();
    assert_eq!(a.run_checksum, b.run_checksum);
}

/// Регресс: опоздавший отчёт воркера по устаревшей эпохе не должен засчитываться
/// в следующем батче. Эмулируем ситуацию «отмена, грейс истёк, reset, новый
/// батч» и убеждаемся, что контрольные суммы нового батча корректны.
#[test]
fn late_worker_report_does_not_corrupt_next_batch() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    engine.reset();
    // Серия отмен «туда-обратно» многократно нагружает окно гонки.
    for _ in 0..10 {
        let canceller = engine.canceller();
        let _ = std::thread::scope(|s| {
            let handled = s.spawn(|| {
                engine.prepare_sample_buffer(Phase::Heavy, 30);
                engine.run_phase(Phase::Heavy, RunTarget::Duration(Duration::from_secs(30)))
            });
            std::thread::sleep(Duration::from_millis(20));
            canceller.store(true, Ordering::Release);
            handled.join().unwrap()
        });
        engine.reset();
    }
    // Финальный эталонный прогон должен совпасть с чистым.
    engine.reset();
    let a = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(64))
        .unwrap();
    engine.reset();
    let b = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(64))
        .unwrap();
    assert_eq!(a.run_checksum, b.run_checksum, "гонка отчётов испортила батч");
    assert_eq!(a.first_tick_checksum, b.first_tick_checksum);
}

/// Порядок фаз в `PHASE_ORDER` обязан совпадать с их индексами.
///
/// От этого инварианта зависят все позиционные массивы: `first_tick_checksums`,
/// `run_checksums`, `StoredRun::phases` и `PhaseStats::phase_index`. При добавлении
/// фазы без проверки индексы разъезжаются, и метрики молча считаются по чужой
/// фазе: так однажды burst retention считался по «Частичной» вместо «Тяжёлой».
#[test]
fn phase_order_matches_phase_indices() {
    for (position, phase) in crate::config::PHASE_ORDER.iter().enumerate() {
        assert_eq!(
            position,
            phase.index() as usize,
            "фаза на позиции {position} имеет индекс {} — массивы разъедутся",
            phase.index()
        );
    }
}

/// Ошибка `SampleCapacityReached`, а не тихое обрезание.
#[test]
fn sample_capacity_reached_is_an_error() {
    let _g = lock();
    let mut engine = Engine::new(Some(2));
    engine.reset();
    engine.prepare_sample_buffer(Phase::Light, 1); // ёмкость 32 000
    let err = engine
        .run_phase(Phase::Light, RunTarget::Ticks(35_000))
        .unwrap_err();
    assert_eq!(err, RunError::SampleCapacityReached { capacity: 32_000 });
}

/// Обязательная самопроверка ядра при старте — проходит и детерминирована.
#[test]
fn self_check_passes_and_fixes_reference_checksums() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    engine.self_check().expect("самопроверка ядра не прошла");
    let light_ref = engine
        .first_tick_reference(Profile::Light)
        .expect("нет эталона Light");
    let heavy_ref = engine
        .first_tick_reference(Profile::Heavy)
        .expect("нет эталона Heavy");
    let response_ref = engine
        .first_tick_reference(Profile::ResponseBase)
        .expect("нет эталона ResponseBase");

    // Фактические первые тики фаз сверяются с эталонами.
    engine.reset();
    let light = engine
        .run_phase(Phase::Light, RunTarget::Ticks(32))
        .unwrap();
    engine.reset();
    let heavy = engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(32))
        .unwrap();
    engine.reset();
    let response = engine
        .run_phase(Phase::Response, RunTarget::Ticks(256))
        .unwrap();

    assert!(engine.verify_first_tick(Profile::Light, light.first_tick_checksum));
    assert!(engine.verify_first_tick(Profile::Heavy, heavy.first_tick_checksum));
    assert!(engine.verify_first_tick(Profile::ResponseBase, response.first_tick_checksum));
    assert_eq!(light_ref, light.first_tick_checksum);
    assert_eq!(heavy_ref, heavy.first_tick_checksum);
    assert_eq!(response_ref, response.first_tick_checksum);
}

/// Регрессия: в фазе «Частичная» половина пула простаивает, и раздача задач
/// должна идти по числу АКТИВНЫХ воркеров.
///
/// Раньше шаг раздачи был равен числу воркеров пула, поэтому задачи с индексами
/// выше `active_workers` не выполнялись вообще, а их слоты сохраняли значения
/// прошлой фазы. Симптом был не в измерениях, а в агрегации: по 5 раундов
/// контрольная сумма фазы «Частичная» отличалась каждый раз, и все схемы
/// отбрасывались с «контрольная сумма различается между повторами». Одно
/// сравнение «Частичная → Тяжёлая → Ч��стичная» это ловит.
#[test]
fn partial_phase_checksum_does_not_depend_on_previous_phase() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    engine.self_check().expect("самопроверка ядра не прошла");

    engine.reset();
    let first = engine
        .run_phase(Phase::Partial, RunTarget::Ticks(32))
        .unwrap();
    // Между прогонами — фаза с полной нагрузкой, которая переписывает все слоты.
    engine.reset();
    engine
        .run_phase(Phase::Heavy, RunTarget::Ticks(32))
        .unwrap();
    engine.reset();
    let second = engine
        .run_phase(Phase::Partial, RunTarget::Ticks(32))
        .unwrap();

    assert_eq!(
        first.first_tick_checksum, second.first_tick_checksum,
        "фаза «Частичная» зависит от предыдущей фазы — задачи теряются"
    );
    assert_eq!(first.run_checksum, second.run_checksum);
}

/// Все измеряемые фазы обязаны иметь эталон первого тика.
///
/// Эталон — это и есть проверка детерминизма на старте сессии: у фазы без него
/// поломка проявится не сразу, а отбросом всей сессии спустя минуты замера.
#[test]
fn every_measured_phase_has_a_first_tick_reference() {
    let _g = lock();
    let mut engine = Engine::new(Some(4));
    engine.self_check().expect("самопроверка ядра не прошла");
    for phase in crate::config::PHASE_ORDER {
        assert!(
            engine.first_tick_reference(phase.first_profile()).is_some(),
            "у фазы {:?} нет эталона первого тика: детерминизм не проверяется",
            phase
        );
    }
}

/// Самопроверка непротиворечива между отдельными движками и запусками.
#[test]
fn self_check_is_deterministic_across_engines() {
    let _g = lock();
    let mut e1 = Engine::new(Some(2));
    let mut e2 = Engine::new(Some(2));
    e1.self_check().unwrap();
    e2.self_check().unwrap();
    for p in [
        Profile::Light,
        Profile::Partial,
        Profile::Heavy,
        Profile::ResponseBase,
    ] {
        assert_eq!(e1.first_tick_reference(p), e2.first_tick_reference(p));
    }
}

/// Синхронизация отчёта: seed и число воркеров фиксированы и совпадают.
#[test]
fn metadata_is_fixed_at_startup() {
    let _g = lock();
    let engine = Engine::new(Some(7));
    assert_eq!(engine.seed(), crate::config::SEED);
    assert_eq!(engine.worker_count(), 7);
    assert_ne!(engine.config_hash(), "");
    assert_eq!(engine.config_hash().len(), 64);
    assert_eq!(engine.version(), crate::config::VERSION);
}
