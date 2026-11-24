# Struve LLM

Struve LLM began as an assistant for the development of Struve-Astro, an astronomy project. It is a Windows desktop app where you can work with a local language model, source code and documents in one chat. For example, you can attach a snippet of image-processing code and ask how it works, or ask a question about a table in a report.

When you need an illustration or a video, you can connect an image or video generation model. Give it a description and reference images in the same chat, and the result appears alongside the conversation.

Each project has its own chats, shared memory and tools. You can also control the local service from the CLI. The interface supports English and Russian. [Русский](README.md)

## Try the alpha

Download the Windows x64 installer from the [alpha release](https://github.com/michailzhivov-spec/StruveLLM/releases/tag/v0.8.2-alpha.1). The release includes `SHA256SUMS.txt` to verify the download.

Choose **English** or **Русский** in the first setup dialog. This selects the language of both the installer and the app on first launch. Reinstalling preserves the language saved in the app.

Run the Windows x64 installer under your own account. Open **Resources** to inspect the machine. In **Models → Connect a local model**, choose a local GGUF chat model and **CPU + RAM**, or connect an existing local Ollama or OpenAI-compatible server. The installer includes a CPU llama.cpp engine. Bring your own compatible chat weights. Closing the window hides it in the tray. Choose **Exit** from the tray to stop the app and its managed models.

The side panel previews documents, source, images and web tabs. Project memory links decisions to their sources across chats. The scheduler tracks memory usage and explains queued jobs. The extension catalog and MCP connections expose document tools, web access and external programs. Image, edit and video jobs can use separate profiles for SD, FLUX and H3 when the corresponding backend and weights are configured. CUDA-enabled engines and generative weights are separate downloads. Scientific model training is outside this alpha.

From the installation directory, run `bin\struve-llm.cmd --json doctor` or `bin\struve-llm.cmd --json resources list`. The packaging tag is **0.8.2-alpha.1** and the application reports **0.8.2**.

[Build and contribute](docs/BUILD.md) · [Second-PC test, in Russian](docs/TEST_SECOND_PC_RU.md) · [MIT license](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)

## Join development

We welcome users and developers who want to shape Struve LLM. You can participate in English or Russian.

- **Have an idea?** Tell us what you were trying to do and how you would like it to work in the app. [Suggest a feature](https://github.com/michailzhivov-spec/StruveLLM/issues/new?template=feature.yml)
- **Tried it on your PC?** We are especially interested in first launch and how jobs behave when memory runs low. Include your Windows version, hardware and steps if something goes wrong. [Report a bug](https://github.com/michailzhivov-spec/StruveLLM/issues/new?template=bug.yml)
- **Want to contribute a change?** Pick an issue or propose your own. Fork the repository, work on a branch and open a pull request against `main`. Code, translations, documentation and example workflows are all welcome. [Open issues](https://github.com/michailzhivov-spec/StruveLLM/issues) · [Build instructions](docs/BUILD.md)
