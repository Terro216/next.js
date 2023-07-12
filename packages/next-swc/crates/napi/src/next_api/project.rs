use std::sync::Arc;

use anyhow::Result;
use napi::{bindgen_prelude::External, JsFunction};
use next_api::{
    project::{EntrypointsOptions, Middleware, ProjectOptions, ProjectVc},
    route::{EndpointVc, Route},
};
use turbo_tasks::TurboTasks;
use turbopack_binding::turbo::tasks_memory::MemoryBackend;

use super::utils::{serde_enum_to_string, subscribe, RootTask, VcArc};
use crate::register;

#[napi(object)]
pub struct NapiProjectOptions {
    /// A root path from which all files must be nested under. Trying to access
    /// a file outside this root will fail. Think of this as a chroot.
    pub root_path: String,

    /// A path inside the root_path which contains the app/pages directories.
    pub project_path: String,

    /// Whether to watch he filesystem for file changes.
    pub watch: bool,

    /// An upper bound of memory that turbopack will attempt to stay under.
    pub memory_limit: Option<f64>,
}

impl Into<ProjectOptions> for NapiProjectOptions {
    fn into(self) -> ProjectOptions {
        ProjectOptions {
            root_path: self.root_path,
            project_path: self.project_path,
            watch: self.watch,
        }
    }
}

#[napi(object)]
pub struct NapiEntrypointsOptions {
    /// File extensions to scan inside our project
    pub page_extensions: Vec<String>,
}

impl Into<EntrypointsOptions> for NapiEntrypointsOptions {
    fn into(self) -> EntrypointsOptions {
        EntrypointsOptions {
            page_extensions: self.page_extensions,
        }
    }
}

#[napi(ts_return_type = "{ __napiType: \"Project\" }")]
pub async fn project_new(options: NapiProjectOptions) -> napi::Result<External<VcArc<ProjectVc>>> {
    register();
    let turbo_tasks = TurboTasks::new(MemoryBackend::new(
        options
            .memory_limit
            .map(|m| m as usize)
            .unwrap_or(usize::MAX),
    ));
    let options = options.into();
    let project = turbo_tasks
        .run_once(async move { Ok(ProjectVc::new(options).resolve().await?) })
        .await?;
    Ok(External::new_with_size_hint(
        VcArc::new(turbo_tasks, project),
        100,
    ))
}

#[napi(object)]
#[derive(Default)]
struct NapiRoute {
    /// The relative path from project_path to the route file
    pub pathname: String,

    /// The type of route, eg a Page or App
    pub r#type: &'static str,

    // Different representations of the endpoint
    pub endpoint: Option<External<VcArc<EndpointVc>>>,
    pub html_endpoint: Option<External<VcArc<EndpointVc>>>,
    pub rsc_endpoint: Option<External<VcArc<EndpointVc>>>,
    pub data_endpoint: Option<External<VcArc<EndpointVc>>>,
}

impl NapiRoute {
    fn from_route(
        pathname: String,
        value: Route,
        turbo_tasks: &Arc<TurboTasks<MemoryBackend>>,
    ) -> Self {
        let convert_endpoint =
            |endpoint: EndpointVc| Some(External::new(VcArc::new(turbo_tasks.clone(), endpoint)));
        match value {
            Route::Page {
                html_endpoint,
                data_endpoint,
            } => NapiRoute {
                pathname,
                r#type: "page",
                html_endpoint: convert_endpoint(html_endpoint.clone()),
                data_endpoint: convert_endpoint(data_endpoint.clone()),
                ..Default::default()
            },
            Route::PageApi { endpoint } => NapiRoute {
                pathname,
                r#type: "page-api",
                endpoint: convert_endpoint(endpoint.clone()),
                ..Default::default()
            },
            Route::AppPage {
                html_endpoint,
                rsc_endpoint,
            } => NapiRoute {
                pathname,
                r#type: "app-page",
                html_endpoint: convert_endpoint(html_endpoint.clone()),
                rsc_endpoint: convert_endpoint(rsc_endpoint.clone()),
                ..Default::default()
            },
            Route::AppRoute { endpoint } => NapiRoute {
                pathname,
                r#type: "app-route",
                endpoint: convert_endpoint(endpoint.clone()),
                ..Default::default()
            },
            Route::Conflict => NapiRoute {
                pathname,
                r#type: "conflict",
                ..Default::default()
            },
        }
    }
}

#[napi(object)]
struct NapiMiddleware {
    pub endpoint: External<VcArc<EndpointVc>>,
    pub runtime: String,
    pub matcher: Option<Vec<String>>,
}

impl NapiMiddleware {
    fn from_middleware(
        value: &Middleware,
        turbo_tasks: &Arc<TurboTasks<MemoryBackend>>,
    ) -> Result<Self> {
        Ok(NapiMiddleware {
            endpoint: External::new(VcArc::new(turbo_tasks.clone(), value.endpoint.clone())),
            runtime: serde_enum_to_string(&value.config.runtime)?,
            matcher: value.config.matcher.clone(),
        })
    }
}

#[napi(object)]
struct NapiEntrypoints {
    pub routes: Vec<NapiRoute>,
    pub middleware: Option<NapiMiddleware>,
}

#[napi(ts_return_type = "{ __napiType: \"RootTask\" }")]
pub fn project_entrypoints_subscribe(
    #[napi(ts_arg_type = "{ __napiType: \"Project\" }")] project: External<VcArc<ProjectVc>>,
    options: NapiEntrypointsOptions,
    func: JsFunction,
) -> napi::Result<External<RootTask>> {
    let turbo_tasks = project.turbo_tasks().clone();
    let project = **project;
    let options: EntrypointsOptions = options.into();
    subscribe(
        turbo_tasks.clone(),
        func,
        move || {
            let options = options.clone();
            async move {
                let entrypoints = project.entrypoints(options).strongly_consistent().await?;
                Ok(entrypoints)
            }
        },
        move |ctx| {
            let entrypoints = ctx.value;
            Ok(vec![NapiEntrypoints {
                routes: entrypoints
                    .routes
                    .iter()
                    .map(|(pathname, &route)| {
                        NapiRoute::from_route(pathname.clone(), route, &turbo_tasks)
                    })
                    .collect::<Vec<_>>(),
                middleware: entrypoints
                    .middleware
                    .as_ref()
                    .map(|m| NapiMiddleware::from_middleware(&m, &turbo_tasks))
                    .transpose()?,
            }])
        },
    )
}
