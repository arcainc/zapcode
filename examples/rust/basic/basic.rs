//! Basic Zapcode example — execute TypeScript from Rust.
//!
//! Run with: cargo run --example basic

use zapcode_core::{ZapcodeRun, ResourceLimits, Value, VmState};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --- 1. Simple expression ---
    let runner = ZapcodeRun::new(
        "1 + 2 * 3".to_string(),
        vec![],
        vec![],
        ResourceLimits::default(),
    )?;
    let result = runner.run_simple()?;
    println!("1 + 2 * 3 = {:?}", result); // Int(7)

    // --- 2. Using inputs ---
    let runner = ZapcodeRun::new(
        r#"
            const greeting = `Hello, ${name}! You are ${age} years old.`;
            greeting
        "#
        .to_string(),
        vec!["name".to_string(), "age".to_string()],
        vec![],
        ResourceLimits::default(),
    )?;
    let result = runner.run(vec![
        ("name".to_string(), Value::String("Zapcode".into())),
        ("age".to_string(), Value::Int(30)),
    ])?;
    println!("Greeting: {:?}", result.state); // Complete("Hello, Zapcode! You are 30 years old.")

    // --- 3. External function (snapshot/resume) ---
    let runner = ZapcodeRun::new(
        r#"
            const weather = await getWeather(city);
            const summary = `Weather in ${city}: ${weather.condition}, ${weather.temp}°C`;
            summary
        "#
        .to_string(),
        vec!["city".to_string()],
        vec!["getWeather".to_string()],
        ResourceLimits::default(),
    )?;

    // Start execution — suspends at getWeather()
    let state = runner.start(vec![
        ("city".to_string(), Value::String("London".into())),
    ])?;

    match state {
        VmState::Suspended {
            function_name,
            args,
            snapshot,
        } => {
            println!("Suspended on: {}({:?})", function_name, args);

            // In a real app, you'd call an actual weather API here.
            // For this example, we return a mock response.
            let weather_data = Value::Object(indexmap::indexmap! {
                "condition".into() => Value::String("Partly cloudy".into()),
                "temp".into() => Value::Int(18),
            });

            // Resume with the mock result
            let final_state = snapshot.resume(weather_data)?;
            match final_state {
                VmState::Complete(value) => {
                    println!("Result: {:?}", value);
                    // "Weather in London: Partly cloudy, 18°C"
                }
                _ => println!("Unexpected second suspension"),
            }
        }
        VmState::Complete(value) => {
            println!("Completed immediately: {:?}", value);
        }
    }

    // --- 4. Snapshot serialization (store and resume later) ---
    let runner = ZapcodeRun::new(
        r#"
            const data = await fetchData(url);
            data.length
        "#
        .to_string(),
        vec!["url".to_string()],
        vec!["fetchData".to_string()],
        ResourceLimits::default(),
    )?;

    let state = runner.start(vec![
        ("url".to_string(), Value::String("https://example.com".into())),
    ])?;

    if let VmState::Suspended { snapshot, .. } = state {
        // Serialize to bytes — store in a database, send over the network, etc.
        let bytes = snapshot.dump()?;
        println!("Snapshot size: {} bytes", bytes.len());

        // Later (possibly in a different process): restore and resume
        let restored = zapcode_core::ZapcodeSnapshot::load(&bytes)?;
        let final_state = restored.resume(Value::String("hello world".into()))?;
        if let VmState::Complete(value) = final_state {
            println!("Restored result: {:?}", value); // Int(11)
        }
    }

    // --- 5. Async map with multiple external calls ---
    // arr.map(async fn => await external()) now works —
    // each external call suspends/resumes sequentially.
    let runner = ZapcodeRun::new(
        r#"
            const cities = ["London", "Tokyo", "Paris"];
            const results = cities.map(async (city) => {
                const weather = await getWeather(city);
                return weather;
            });
            results
        "#
        .to_string(),
        vec![],
        vec!["getWeather".to_string()],
        ResourceLimits::default(),
    )?;

    let mut state = runner.start(vec![])?;

    // The VM suspends once per city — resolve each one
    let mock_data = vec![
        ("London", "Rainy, 12°C"),
        ("Tokyo", "Clear, 26°C"),
        ("Paris", "Sunny, 22°C"),
    ];

    for (expected_city, weather) in &mock_data {
        match state {
            VmState::Suspended {
                function_name,
                args,
                snapshot,
            } => {
                println!(
                    "  -> {}({}) = {}",
                    function_name,
                    args[0].to_js_string(),
                    weather
                );
                assert_eq!(function_name, "getWeather");
                assert_eq!(args[0].to_js_string(), *expected_city);
                state = snapshot.resume(Value::String((*weather).into()))?;
            }
            VmState::Complete(_) => panic!("expected suspension for {}", expected_city),
        }
    }

    match state {
        VmState::Complete(value) => {
            println!("Async map result: {:?}", value);
            // Array(["Rainy, 12°C", "Clear, 26°C", "Sunny, 22°C"])
        }
        _ => println!("Unexpected suspension after all cities resolved"),
    }

    Ok(())
}
