// Jaeger UI configuration for the loopback fixture evidence reader.
function UIConfig() {
  return {
    linkPatterns: [{
      type: "attributes",
      key: "gaap.evidence.ref",
      value: /^receipts\/[0-9a-f]{64}$/,
      url: {
        parameters: ["gaap.evidence.ref"],
        template: values => "http://127.0.0.1:19490/" + values["gaap.evidence.ref"]
      },
      text: "Open fixture evidence"
    }]
  }
}
